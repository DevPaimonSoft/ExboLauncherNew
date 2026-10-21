use super::*;
use std::{
    io::Read,
    sync::{atomic::AtomicUsize, mpsc},
};

pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub entry: Option<Measured>,
}

fn fetch(region: &str, control: &Control) -> Result<Catalog> {
    let sources: &[&str] = match region {
        "ru" => &[
            "https://backend.stalzone.net/address_list",
            "https://backend.stalzone.com/address_list",
            "https://backend.stalcraftx.ru/address_list",
        ],
        "eu" => &[
            "https://backend-eu.stalzone.com/address_list",
            "https://backend-eu.stalzone.net/address_list",
        ],
        "na" => &[
            "https://backend-na.stalzone.com/address_list",
            "https://backend-na.stalzone.net/address_list",
        ],
        "sea" => &[
            "https://backend-sea.stalzone.com/address_list",
            "https://backend-sea.stalzone.net/address_list",
        ],
        "nea" => &[
            "https://backend-nea.stalzone.com/address_list",
            "https://backend-nea.stalzone.net/address_list",
        ],
        _ => anyhow::bail!("Неизвестный регион"),
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    for source in sources {
        control.check()?;
        let attempt = (|| -> Result<Catalog> {
            let mut bytes = vec![];
            client
                .get(*source)
                .query(&[("login", "EXBO-Community")])
                .send()?
                .error_for_status()?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() <= 1024 * 1024,
                "Слишком большой список туннелей"
            );
            let catalog: Catalog = serde_json::from_slice(&bytes)?;
            validate_catalog(&catalog)?;
            Ok(catalog)
        })();
        control.check()?;
        if let Ok(catalog) = attempt {
            return Ok(catalog);
        }
    }
    anyhow::bail!(
        "Не удалось получить список туннелей {region}. Повторите позже; текущие настройки сохранены"
    )
}

pub fn measure(
    region: String,
    mode: u8,
    excluded: Vec<String>,
    cache: Cache,
    control: Control,
    progress: impl Fn(Progress) + Send + Sync,
) -> Result<Results> {
    let catalog = fetch(&region, &control)?;
    let mut seen = HashSet::new();
    let mut targets = vec![];
    for pool in catalog.pools {
        if excluded.contains(&pool.name) {
            continue;
        }
        for endpoint in pool.tunnels {
            if seen.insert(endpoint.address.clone()) {
                targets.push(Target {
                    pool: pool.name.clone(),
                    endpoint,
                });
            }
        }
    }
    ensure!(
        !targets.is_empty(),
        "Все группы исключены или в регионе нет доступных узлов"
    );
    measure_targets(
        region,
        mode,
        catalog.client_to_tunnel_rtt_weight,
        targets,
        cache,
        &control,
        progress,
    )
}

pub(super) fn measure_targets(
    region: String,
    mode: u8,
    weight: f64,
    targets: Vec<Target>,
    mut cache: Cache,
    control: &Control,
    progress: impl Fn(Progress) + Send + Sync,
) -> Result<Results> {
    control.check()?;
    let total = targets.len();
    progress(Progress {
        done: 0,
        total,
        entry: None,
    });
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();
    let entries = std::thread::scope(|scope| -> Result<Vec<Measured>> {
        for _ in 0..CONCURRENCY.min(total) {
            let (tx, targets, next) = (tx.clone(), &targets, &next);
            scope.spawn(move || {
                while !control.cancelled() {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(target) = targets.get(index) else {
                        break;
                    };
                    let mut samples = Vec::with_capacity(PROBE_COUNT);
                    let mut limited = false;
                    for attempt in 0..PROBE_COUNT {
                        if control.check().is_err()
                            || (attempt > 0 && control.pause(PROBE_INTERVAL).is_err())
                        {
                            return;
                        }
                        let result =
                            probe_with_control(&target.endpoint.address, PROBE_TIMEOUT, control);
                        if control.cancelled() {
                            return;
                        }
                        let sample = match result {
                            Ok(p) => {
                                limited = p.limited;
                                Sample {
                                    time: now(),
                                    client: p.client,
                                    server: p.server,
                                    failed: false,
                                }
                            }
                            Err(_) => Sample {
                                time: now(),
                                failed: true,
                                ..Default::default()
                            },
                        };
                        samples.push(sample);
                    }
                    if tx.send((target.clone(), samples, limited)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut entries = vec![];
        for (target, samples, limited) in rx {
            control.check()?;
            let entry = cache.record_round(&region, target, &samples, mode, weight, limited);
            entries.push(entry.clone());
            progress(Progress {
                done: entries.len(),
                total,
                entry: Some(entry),
            });
        }
        control.check()?;
        Ok(entries)
    })?;
    let mut entries = entries;
    sort(&mut entries);
    cache.prune();
    Ok(Results {
        region,
        entries,
        cache,
    })
}
