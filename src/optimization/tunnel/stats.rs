use super::*;
use std::{collections::BTreeMap, io::Read, path::Path};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Sample {
    pub time: u64,
    pub client: f64,
    pub server: f64,
    pub failed: bool,
}
impl Sample {
    fn valid(&self, time: u64, age: u64) -> bool {
        self.time > 0
            && self.time <= time
            && time - self.time <= age
            && self.client.is_finite()
            && self.client >= 0.
            && self.server.is_finite()
            && self.server >= 0.
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Cache {
    pub samples: BTreeMap<String, Vec<Sample>>,
}
#[derive(Clone, Debug)]
pub struct Measured {
    pub target: Target,
    pub client: f64,
    pub server: f64,
    pub jitter: f64,
    pub loss: f64,
    pub score: f64,
    pub median: f64,
    pub p90: f64,
    pub max: f64,
    pub last_client: f64,
    pub last_server: f64,
    pub samples: usize,
    pub successes: usize,
    pub available: bool,
    pub limited: bool,
    observations: Vec<Sample>,
    weight: f64,
}
impl Measured {
    pub fn for_mode(&self, mode: u8) -> Self {
        super::rank(
            self.target.clone(),
            &self.observations,
            mode,
            self.weight,
            self.available,
            self.limited,
        )
    }
    pub fn eligible(&self) -> bool {
        self.available && self.successes >= MIN_REPLIES
    }
}
pub fn best_index(entries: &[Measured]) -> Option<usize> {
    entries.iter().position(Measured::eligible)
}
pub fn sort(entries: &mut [Measured]) {
    entries.sort_by(|a, b| {
        a.score
            .total_cmp(&b.score)
            .then(a.target.pool.cmp(&b.target.pool))
            .then(a.target.endpoint.name.cmp(&b.target.endpoint.name))
            .then(a.target.endpoint.address.cmp(&b.target.endpoint.address))
    });
}

impl Cache {
    pub fn read(path: &Path) -> Self {
        let read = (|| -> Result<Self> {
            let mut bytes = vec![];
            std::fs::File::open(path)?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 4 * 1024 * 1024, "Кэш слишком велик");
            Ok(serde_json::from_slice(&bytes)?)
        })();
        let mut cache = read.unwrap_or_default();
        cache.prune();
        cache
    }
    pub fn prune(&mut self) {
        let time = now();
        self.samples.retain(|_, samples| {
            samples.retain(|s| s.valid(time, 86400));
            samples.sort_by_key(|s| s.time);
            if samples.len() > 20 {
                samples.drain(..samples.len() - 20);
            }
            !samples.is_empty()
        });
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = serde_json::to_vec(self)?;
        ensure!(bytes.len() <= 4 * 1024 * 1024, "Кэш туннелей слишком велик");
        crate::storage::atomic_write(path, &bytes)
    }
    pub(super) fn record_round(
        &mut self,
        region: &str,
        target: Target,
        samples: &[Sample],
        mode: u8,
        weight: f64,
        limited: bool,
    ) -> Measured {
        let result = rank(target.clone(), samples, mode, weight, true, limited);
        let history = self
            .samples
            .entry(format!("{region}|{}", target.endpoint.address))
            .or_default();
        history.extend_from_slice(samples);
        if history.len() > 20 {
            history.drain(..history.len() - 20);
        }
        result
    }
}

fn quantiles(values: &mut [f64]) -> (f64, f64) {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    let median = if values.len().is_multiple_of(2) {
        values[middle - 1] + (values[middle] - values[middle - 1]) / 2.
    } else {
        values[middle]
    };
    (median, values[(values.len() * 9).div_ceil(10) - 1])
}
pub fn rank(
    target: Target,
    samples: &[Sample],
    mode: u8,
    weight: f64,
    available: bool,
    limited: bool,
) -> Measured {
    let time = now();
    let mut recent: Vec<_> = samples
        .iter()
        .filter(|s| s.valid(time, 3600))
        .cloned()
        .collect();
    recent.sort_by_key(|s| s.time);
    let success: Vec<_> = recent.iter().filter(|s| !s.failed).collect();
    let count = success.len().max(1) as f64;
    let client = success.iter().map(|s| s.client).sum::<f64>() / count;
    let server = success.iter().map(|s| s.server).sum::<f64>() / count;
    let loss = if recent.is_empty() {
        1.
    } else {
        (recent.len() - success.len()) as f64 / recent.len() as f64
    };
    let mut totals: Vec<_> = success.iter().map(|s| s.client + s.server).collect();
    let jitter = if totals.len() < 2 {
        0.
    } else {
        totals.windows(2).map(|p| (p[1] - p[0]).abs()).sum::<f64>() / (totals.len() - 1) as f64
    };
    let (median, p90, max) = if totals.is_empty() {
        (0., 0., 0.)
    } else {
        let (median, p90) = quantiles(&mut totals);
        (median, p90, *totals.last().unwrap())
    };
    let available = available
        && !limited
        && !success.is_empty()
        && mode <= 4
        && (mode != 4 || (weight.is_finite() && weight >= 0.));
    let mut score = f64::INFINITY;
    if available {
        let mut values: Vec<_> = success
            .iter()
            .map(|s| match mode {
                1 => s.client,
                2 => s.server,
                4 => s.client * weight + s.server,
                _ => s.client + s.server,
            })
            .collect();
        if values.iter().all(|v| v.is_finite() && *v >= 0.) {
            let (mid, tail) = quantiles(&mut values);
            let value = if mode == 3 {
                tail
            } else {
                mid + 0.5 * (tail - mid)
            } + PROBE_TIMEOUT.as_secs_f64() * 1000. * loss;
            if value.is_finite() && value >= 0. {
                score = value;
            }
        }
    }
    let (last_client, last_server) = success.last().map_or((0., 0.), |s| (s.client, s.server));
    Measured {
        target,
        client,
        server,
        jitter,
        loss,
        score,
        median,
        p90,
        max,
        last_client,
        last_server,
        samples: recent.len(),
        successes: success.len(),
        available: available && score.is_finite(),
        limited,
        observations: recent,
        weight,
    }
}
