use super::JvmProfile;

pub(super) fn base_flags(p: &JvmProfile) -> Vec<String> {
    let cc = if p.reserved_code_cache_size_mb == 0 {
        256
    } else {
        p.reserved_code_cache_size_mb
    };
    vec![
        format!("-Xmx{}g", p.heap_size_gb),
        format!("-Xms{}g", p.heap_size_gb),
        format!("-XX:MetaspaceSize={}m", p.metaspace_mb),
        format!("-XX:MaxMetaspaceSize={}m", p.metaspace_mb),
        "-XX:+UseG1GC".to_string(),
        "-XX:+UnlockExperimentalVMOptions".to_string(),
        format!("-XX:MaxGCPauseMillis={}", p.max_gc_pause_millis),
        format!("-XX:G1HeapRegionSize={}m", p.g1_heap_region_size_mb),
        format!("-XX:G1NewSizePercent={}", p.g1_new_size_percent),
        format!("-XX:G1MaxNewSizePercent={}", p.g1_max_new_size_percent),
        format!("-XX:G1ReservePercent={}", p.g1_reserve_percent),
        format!("-XX:G1HeapWastePercent={}", p.g1_heap_waste_percent),
        format!("-XX:G1MixedGCCountTarget={}", p.g1_mixed_gc_count_target),
        "-XX:+G1UseAdaptiveIHOP".to_string(),
        format!(
            "-XX:InitiatingHeapOccupancyPercent={}",
            p.initiating_heap_occupancy_percent
        ),
        format!(
            "-XX:G1MixedGCLiveThresholdPercent={}",
            p.g1_mixed_gc_live_threshold_percent
        ),
        format!(
            "-XX:G1RSetUpdatingPauseTimePercent={}",
            p.g1_rset_updating_pause_time_percent
        ),
        format!("-XX:SurvivorRatio={}", p.survivor_ratio),
        format!("-XX:MaxTenuringThreshold={}", p.max_tenuring_threshold),
        format!("-XX:ParallelGCThreads={}", p.parallel_gc_threads),
        format!("-XX:ConcGCThreads={}", p.conc_gc_threads),
        "-XX:+ParallelRefProcEnabled".to_string(),
        "-XX:+DisableExplicitGC".to_string(),
        format!(
            "-XX:SoftRefLRUPolicyMSPerMB={}",
            p.soft_ref_lru_policy_ms_per_mb
        ),
        "-XX:-UseBiasedLocking".to_string(),
        "-XX:+DisableAttachMechanism".to_string(),
        format!("-XX:ReservedCodeCacheSize={}m", cc),
        format!("-XX:NonNMethodCodeHeapSize={}m", cc * 5 / 100),
        format!("-XX:ProfiledCodeHeapSize={}m", cc * 48 / 100),
        format!(
            "-XX:NonProfiledCodeHeapSize={}m",
            cc - cc * 5 / 100 - cc * 48 / 100
        ),
        format!("-XX:MaxInlineLevel={}", p.max_inline_level),
        format!("-XX:FreqInlineSize={}", p.freq_inline_size),
        "-Djdk.nio.maxCachedBufferSize=131072".to_string(),
    ]
}
