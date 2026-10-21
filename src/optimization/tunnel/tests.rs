use super::*;

fn target(address: &str) -> Target {
    Target {
        pool: "test".into(),
        endpoint: Endpoint {
            name: address.into(),
            address: address.into(),
        },
    }
}
fn sample(client: f64, server: f64) -> Sample {
    Sample {
        time: now(),
        client,
        server,
        failed: false,
    }
}

#[test]
fn failures_are_penalized_in_every_search_mode() {
    let failed = Sample {
        time: now(),
        failed: true,
        ..Default::default()
    };
    for mode in 0..=4 {
        let unstable = rank(
            target("unstable:2000"),
            &[sample(5., 5.), failed.clone(), failed.clone()],
            mode,
            1.,
            true,
            false,
        );
        let stable = rank(
            target("stable:2000"),
            &[sample(20., 20.)],
            mode,
            1.,
            true,
            false,
        );
        assert!(
            unstable.score > stable.score,
            "mode {mode} ignored failed replies"
        );
        assert_eq!(unstable.successes, 1);
        assert!(!unstable.eligible());
    }
    let lost = rank(target("lost:2000"), &[failed], 0, 1., true, false);
    assert!(!lost.available && lost.score.is_infinite());
}
#[test]
fn pr47_quantiles_use_paired_measurements_in_each_mode() {
    let samples = [sample(0., 100.), sample(10., 0.), sample(100., 10.)];
    for (mode, expected) in [(0, 105.), (1, 55.), (2, 55.), (3, 110.), (4, 155.)] {
        let result = rank(target("paired:2000"), &samples, mode, 2., true, false);
        assert_eq!(result.score, expected, "mode {mode}");
        assert_eq!((result.median, result.p90), (100., 110.));
    }
}
#[test]
fn a_large_outlier_remains_visible_without_deciding_the_winner() {
    let mut samples = vec![sample(10., 15.); 10];
    samples[9].server = 2_000_000.;
    for (mode, score) in [(0, 25.), (1, 10.), (2, 15.), (3, 25.), (4, 30.)] {
        let result = rank(target("outlier:2000"), &samples, mode, 1.5, true, false);
        assert_eq!(result.score, score);
        assert_eq!(
            (result.median, result.p90, result.max),
            (25., 25., 2_000_010.)
        );
        assert_eq!(result.last_server, 2_000_000.);
        assert_eq!(result.server, 200_013.5);
        assert_eq!(samples[9].server, 2_000_000.);
    }
}
#[test]
fn fresh_round_does_not_reuse_historical_outliers_or_limits() {
    let node = target("fresh:2000");
    let mut cache = Cache::default();
    cache
        .samples
        .insert("ru|fresh:2000".into(), vec![sample(10., 2_000_000.); 10]);
    let result = cache.record_round("ru", node, &vec![sample(10., 15.); 10], 0, 1., false);
    assert_eq!(result.score, 25.);
    assert_eq!(cache.samples["ru|fresh:2000"].len(), 20);
    assert_eq!(cache.samples["ru|fresh:2000"][0].server, 2_000_000.);
    assert!(result.eligible());
    let limited = rank(
        target("limited:2000"),
        &vec![sample(1., 1.); 10],
        0,
        1.,
        true,
        true,
    );
    assert!(!limited.available && !limited.eligible());
}
#[test]
fn best_skips_under_sampled_and_unavailable_nodes() {
    let mut results = vec![
        rank(target("fast:2000"), &[sample(1., 1.)], 0, 1., true, false),
        rank(
            target("stable:2000"),
            &vec![sample(10., 10.); 5],
            0,
            1.,
            true,
            false,
        ),
    ];
    sort(&mut results);
    assert_eq!(best_index(&results), Some(1));
    assert_eq!(results[1].for_mode(4).score, 20.);
    results[1].available = false;
    assert_eq!(best_index(&results), None);
    for weight in [-1., f64::NAN, f64::INFINITY] {
        assert!(
            !rank(
                target("bad:2000"),
                &[sample(1., 1.)],
                4,
                weight,
                true,
                false
            )
            .available
        );
    }
}
#[test]
fn cancellation_interrupts_an_in_flight_udp_read() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let address = format!("127.0.0.1:{}", socket.local_addr().unwrap().port() - 1);
    let control = Control::default();
    let worker_control = control.clone();
    let worker = std::thread::spawn(move || {
        probe_with_control(&address, Duration::from_secs(5), &worker_control)
    });
    socket.recv_from(&mut [0; 16]).unwrap();
    let started = Instant::now();
    control.cancel();
    let error = worker.join().unwrap().err().unwrap().to_string();
    assert!(error.contains("отменено"), "{error}");
    assert!(started.elapsed() < Duration::from_millis(500));
}
#[test]
fn a_local_search_runs_ten_fresh_probes_and_reports_completed_nodes() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let address = format!("127.0.0.1:{}", socket.local_addr().unwrap().port() - 1);
    let server = std::thread::spawn(move || {
        for index in 0..PROBE_COUNT {
            let mut token = [0; 16];
            let (size, peer) = socket.recv_from(&mut token).unwrap();
            assert_eq!(size, 16);
            let mut response = token.to_vec();
            response.extend((if index == 9 { 2_000_000i32 } else { 15i32 }).to_be_bytes());
            response.push(0);
            socket.send_to(&response, peer).unwrap();
        }
    });
    let progress = std::sync::Mutex::new(Vec::new());
    let results = measure::measure_targets(
        "ru".into(),
        2,
        1.,
        vec![target(&address)],
        Cache::default(),
        &Control::default(),
        |p| {
            progress
                .lock()
                .unwrap()
                .push((p.done, p.total, p.entry.is_some()))
        },
    )
    .unwrap();
    server.join().unwrap();
    assert_eq!(results.entries.len(), 1);
    assert_eq!(
        (results.entries[0].samples, results.entries[0].successes),
        (10, 10)
    );
    assert_eq!(results.entries[0].score, 15.);
    assert!(results.entries[0].max > 2_000_000.);
    assert_eq!(*progress.lock().unwrap(), vec![(0, 1, false), (1, 1, true)]);
    let cancelled = Control::default();
    cancelled.cancel();
    assert!(
        measure::measure_targets(
            "ru".into(),
            0,
            1.,
            vec![target(&address)],
            Cache::default(),
            &cancelled,
            |_| panic!("Cancelled search reported progress")
        )
        .is_err()
    );
}
