pub(crate) mod cpu;
mod flags;
mod profile;
#[cfg(test)]
mod tests;
pub(crate) mod tunnel;
use anyhow::{Context, Result, ensure};
use flags::base_flags;
pub use profile::JvmProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    io::Read,
    path::Path,
    sync::OnceLock,
};

pub const SOURCE: &str = "https://github.com/EXBO-Community/stalcraft-jvm-optimization";
const MAX_PROFILE: u64 = 256 * 1024;
pub const REGIONS: [&str; 5] = ["ru", "eu", "na", "sea", "nea"];

#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub cpu_affinity: Option<Vec<u16>>,
    pub name: String,
    pub profile: Option<JvmProfile>,
    pub tunnels: BTreeMap<String, String>,
    pub search_mode: u8,
    pub excluded_pools: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Portable {
    format: String,
    version: u32,
    settings: Settings,
}

#[derive(Deserialize)]
pub struct Parameter {
    pub key: String,
    pub label_ru: String,
    pub label_en: String,
    pub category: i32,
    pub minimum: f64,
    pub maximum: f64,
    pub unit: String,
    pub help_ru: String,
    pub help_en: String,
    pub boolean: bool,
    pub decimal: bool,
}
pub fn parameters() -> &'static [Parameter] {
    static DATA: OnceLock<Vec<Parameter>> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/optimization/parameters.json"))
            .expect("built-in parameter catalog")
    })
}

#[derive(Clone, Copy, Debug)]
pub struct Hardware {
    pub total_mb: u64,
    pub free_mb: u64,
    pub threads: usize,
    pub memory_mts: u32,
    pub large_pages: bool,
}
impl Hardware {
    pub fn detect() -> Self {
        let mut info = Self {
            total_mb: 0,
            free_mb: 0,
            threads: std::thread::available_parallelism().map_or(4, usize::from),
            memory_mts: 0,
            large_pages: false,
        };
        #[cfg(windows)]
        {
            use windows::Win32::System::SystemInformation::*;
            let mut memory = MEMORYSTATUSEX {
                dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() {
                info.total_mb = memory.ullTotalPhys / 1024 / 1024;
                info.free_mb = memory.ullAvailPhys / 1024 / 1024;
            }
            let size =
                unsafe { GetSystemFirmwareTable(FIRMWARE_TABLE_PROVIDER(0x52534d42), 0, None) };
            if (8..=16 * 1024 * 1024).contains(&size) {
                let mut buffer = vec![0; size as usize];
                let written = unsafe {
                    GetSystemFirmwareTable(
                        FIRMWARE_TABLE_PROVIDER(0x52534d42),
                        0,
                        Some(&mut buffer),
                    )
                };
                if written > 0 && written <= size {
                    info.memory_mts = memory_speed(&buffer[..written as usize]);
                }
            }
            info.large_pages = large_page_privilege();
        }
        info
    }
    pub fn description(self, en: bool) -> String {
        let speed = if self.memory_mts == 0 {
            if en {
                "memory speed unknown".into()
            } else {
                "скорость памяти не определена".into()
            }
        } else {
            format!("{} MT/s", self.memory_mts)
        };
        if en {
            format!(
                "RAM {:.1} GiB · free {:.1} GiB · {} CPU threads · {speed} · large pages: {}",
                self.total_mb as f64 / 1024.,
                self.free_mb as f64 / 1024.,
                self.threads,
                if self.large_pages {
                    "available"
                } else {
                    "unavailable"
                }
            )
        } else {
            format!(
                "RAM {:.1} ГиБ · свободно {:.1} ГиБ · {} потоков CPU · {speed} · большие страницы: {}",
                self.total_mb as f64 / 1024.,
                self.free_mb as f64 / 1024.,
                self.threads,
                if self.large_pages {
                    "доступны"
                } else {
                    "недоступны"
                }
            )
        }
    }
}

pub(crate) fn memory_speed(raw: &[u8]) -> u32 {
    if raw.len() < 8 {
        return 0;
    }
    let length = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
    let Some(table) = raw.get(8..8 + length) else {
        return 0;
    };
    let (mut offset, mut best) = (0, 0);
    while let Some(header) = table.get(offset..offset + 4) {
        let size = header[1] as usize;
        if size < 4 {
            break;
        }
        let Some(record) = table.get(offset..offset + size) else {
            break;
        };
        if header[0] == 127 {
            break;
        }
        if header[0] == 17 && record.len() >= 14 && record[12..14] != [0, 0] {
            for (short, extended) in [(0x20, 0x58), (0x15, 0x54)] {
                if let Some(bytes) = record.get(short..short + 2) {
                    let value = u16::from_le_bytes(bytes.try_into().unwrap());
                    let value = if value == 0xffff {
                        record
                            .get(extended..extended + 4)
                            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                            .unwrap_or(0)
                    } else {
                        value as u32
                    };
                    if value > 0 {
                        best = best.max(value);
                        break;
                    }
                }
            }
        }
        offset += size;
        while offset + 1 < table.len() && table[offset..offset + 2] != [0, 0] {
            offset += 1;
        }
        offset += 2;
    }
    best
}

#[cfg(windows)]
fn large_page_privilege() -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetLargePageMinimum() -> usize;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(
            p: *mut std::ffi::c_void,
            access: u32,
            token: *mut *mut std::ffi::c_void,
        ) -> i32;
        fn LookupPrivilegeValueW(system: *const u16, name: *const u16, luid: *mut u64) -> i32;
        fn GetTokenInformation(
            token: *mut std::ffi::c_void,
            class: u32,
            data: *mut u8,
            size: u32,
            needed: *mut u32,
        ) -> i32;
    }
    unsafe {
        if GetLargePageMinimum() == 0 {
            return false;
        }
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return false;
        }
        let result = (|| {
            let name: Vec<u16> = "SeLockMemoryPrivilege"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let (mut luid, mut needed) = (0u64, 0u32);
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
                return false;
            }
            GetTokenInformation(token, 3, std::ptr::null_mut(), 0, &mut needed);
            if !(4..=65536).contains(&needed) {
                return false;
            }
            let mut data = vec![0; needed as usize];
            if GetTokenInformation(token, 3, data.as_mut_ptr(), needed, &mut needed) == 0 {
                return false;
            }
            let count = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
            data[4..]
                .chunks_exact(12)
                .take(count)
                .any(|v| u64::from_le_bytes(v[..8].try_into().unwrap()) == luid)
        })();
        CloseHandle(token);
        result
    }
}

pub fn automatic(hw: Hardware) -> JvmProfile {
    let mut p = aikar();
    let gb = hw.total_mb / 1024;
    p.heap_size_gb = if gb >= 16 {
        6
    } else if gb >= 12 {
        5
    } else if gb >= 8 {
        4
    } else if gb >= 6 {
        3
    } else {
        2
    };
    p.pre_touch = gb >= 12;
    p.g1_heap_region_size_mb = if p.heap_size_gb <= 3 { 4 } else { 8 };
    let slow = hw.memory_mts > 0 && hw.memory_mts <= 2933;
    p.max_gc_pause_millis = if slow { 150 } else { 100 };
    p.g1_new_size_percent = if slow { 30 } else { 33 };
    p.g1_max_new_size_percent = 50;
    p.g1_reserve_percent = 15;
    p.g1_heap_waste_percent = 10;
    p.g1_mixed_gc_count_target = if slow { 4 } else { 6 };
    p.initiating_heap_occupancy_percent = 25;
    p.g1_mixed_gc_live_threshold_percent = 85;
    p.g1_rset_updating_pause_time_percent = if slow { 5 } else { 8 };
    p.survivor_ratio = 12;
    p.max_tenuring_threshold = 3;
    p.parallel_gc_threads = (hw.threads.saturating_sub(2) as i64)
        .clamp(2, 10)
        .min(hw.threads.max(1) as i64);
    p.conc_gc_threads = (p.parallel_gc_threads / 2).clamp(1, 5);
    p.use_large_pages = hw.large_pages && gb >= 16;
    p
}
pub fn aikar() -> JvmProfile {
    static PROFILE: OnceLock<JvmProfile> = OnceLock::new();
    PROFILE
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../assets/optimization/aikar.json"))
                .expect("built-in Aikar profile")
        })
        .clone()
}

impl JvmProfile {
    pub fn validate(&self) -> Result<()> {
        let values = serde_json::to_value(self)?;
        for field in parameters().iter().filter(|f| !f.boolean) {
            let number = values[&field.key]
                .as_f64()
                .context("Некорректный числовой параметр")?;
            ensure!(
                number.is_finite() && number >= field.minimum && number <= field.maximum,
                "{} ({}): допустимо {}…{}",
                field.label_ru,
                field.key,
                field.minimum,
                field.maximum
            );
        }
        ensure!(
            [1, 2, 4, 8, 16, 32].contains(&self.g1_heap_region_size_mb),
            "Размер региона G1: выберите 1, 2, 4, 8, 16 или 32 МиБ"
        );
        ensure!(
            self.g1_new_size_percent <= self.g1_max_new_size_percent,
            "Минимум молодого поколения не может быть больше максимума"
        );
        ensure!(
            self.conc_gc_threads <= self.parallel_gc_threads,
            "Фоновых потоков GC не должно быть больше параллельных"
        );
        ensure!(
            self.reserved_code_cache_size_mb == 0 || self.reserved_code_cache_size_mb >= 64,
            "Кэш JIT: 0 или не менее 64 МиБ"
        );
        ensure!(
            (self.max_node_limit == 0 && self.node_limit_fudge_factor == 0)
                || (self.max_node_limit >= 1000
                    && self.node_limit_fudge_factor > 0
                    && self.node_limit_fudge_factor < self.max_node_limit / 2),
            "MaxNodeLimit и NodeLimitFudgeFactor задаются вместе; запас должен быть меньше половины лимита"
        );
        Ok(())
    }
    pub fn validate_hardware(&self, hw: Hardware) -> Result<()> {
        self.validate()?;
        ensure!(
            cfg!(target_pointer_width = "64"),
            "Оптимизация JVM требует 64-битного лаунчера и Java"
        );
        ensure!(
            hw.total_mb > 0,
            "Не удалось определить RAM; повторите автоподбор"
        );
        let heap = self.heap_size_gb as u64 * 1024;
        let cache = if self.reserved_code_cache_size_mb == 0 {
            256
        } else {
            self.reserved_code_cache_size_mb as u64
        };
        ensure!(
            heap + self.metaspace_mb as u64 + cache + 512 + 1024 <= hw.total_mb,
            "Профилю недостаточно RAM: уменьшите heap, metaspace или кэш кода (RAM {:.1} ГиБ)",
            hw.total_mb as f64 / 1024.
        );
        if self.use_large_pages {
            ensure!(
                hw.large_pages,
                "Большие страницы недоступны: выключите параметр или назначьте в Windows право «Блокировка страниц в памяти»"
            );
            ensure!(
                hw.total_mb >= 16 * 1024 && heap <= hw.total_mb / 2,
                "Большие страницы требуют минимум 16 ГиБ RAM и heap не больше половины RAM"
            );
        }
        Ok(())
    }
    pub fn flags(&self) -> Vec<String> {
        let mut flags = base_flags(self);
        macro_rules! number {
            ($key:ident,$flag:literal) => {
                if self.$key > 0 {
                    flags.push(format!(concat!($flag, "={}"), self.$key));
                }
            };
        }
        macro_rules! toggle {
            ($key:ident,$flag:literal) => {
                flags.push(format!(
                    "-XX:{}{}",
                    if self.$key { "+" } else { "-" },
                    $flag
                ));
            };
        }
        toggle!(pre_touch, "AlwaysPreTouch");
        number!(
            g1_satb_buffer_enqueuing_threshold_percent,
            "-XX:G1SATBBufferEnqueueingThresholdPercent"
        );
        number!(g1_conc_rs_hot_card_limit, "-XX:G1ConcRSHotCardLimit");
        number!(
            g1_conc_refinement_service_interval_millis,
            "-XX:G1ConcRefinementServiceIntervalMillis"
        );
        number!(gc_time_ratio, "-XX:GCTimeRatio");
        toggle!(
            use_dynamic_number_of_gc_threads,
            "UseDynamicNumberOfGCThreads"
        );
        toggle!(use_string_deduplication, "UseStringDeduplication");
        number!(inline_small_code, "-XX:InlineSmallCode");
        if self.max_node_limit > 0 && self.node_limit_fudge_factor > 0 {
            flags.push(format!(
                "-XX:NodeLimitFudgeFactor={}",
                self.node_limit_fudge_factor
            ));
            flags.push(format!("-XX:MaxNodeLimit={}", self.max_node_limit));
        }
        number!(nmethod_sweep_activity, "-XX:NmethodSweepActivity");
        toggle!(dont_compile_huge_methods, "DontCompileHugeMethods");
        number!(allocate_prefetch_style, "-XX:AllocatePrefetchStyle");
        toggle!(always_act_as_server_class, "AlwaysActAsServerClassMachine");
        toggle!(use_xmm_for_array_copy, "UseXMMForArrayCopy");
        toggle!(use_fpu_for_spilling, "UseFPUForSpilling");
        toggle!(use_large_pages, "UseLargePages");
        flags.push(format!(
            "-Dsun.reflect.inflationThreshold={}",
            self.reflection_inflation_threshold
        ));
        number!(auto_box_cache_max, "-XX:AutoBoxCacheMax");
        toggle!(use_thread_priorities, "UseThreadPriorities");
        if self.use_thread_priorities {
            number!(thread_priority_policy, "-XX:ThreadPriorityPolicy");
        }
        toggle!(use_counter_decay, "UseCounterDecay");
        if self.compile_threshold_scaling > 0. && self.compile_threshold_scaling != 1. {
            flags.push(format!(
                "-XX:CompileThresholdScaling={}",
                self.compile_threshold_scaling
            ));
        }
        flags
    }
}

impl Settings {
    pub fn profile_hardware(&self, mut hw: Hardware) -> Hardware {
        if let Some(cpus) = &self.cpu_affinity {
            hw.threads = cpus.len().max(1);
        }
        hw
    }
    pub fn profile_for(&self, hw: Hardware) -> JvmProfile {
        self.profile
            .clone()
            .unwrap_or_else(|| automatic(self.profile_hardware(hw)))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.name.chars().count() <= 80 && !self.name.chars().any(char::is_control),
            "Название профиля: не более 80 символов без управляющих знаков"
        );
        cpu::validate(self.cpu_affinity.as_deref())?;
        if let Some(profile) = &self.profile {
            profile.validate()?;
        }
        ensure!(self.search_mode <= 4, "Неизвестный способ выбора туннелей");
        for (region, address) in &self.tunnels {
            ensure!(
                REGIONS.contains(&region.as_str()),
                "Неизвестный регион туннеля: {region}"
            );
            tunnel::parse_address(address)?;
        }
        for (region, pools) in &self.excluded_pools {
            ensure!(
                REGIONS.contains(&region.as_str()) && pools.len() <= 100,
                "Некорректный список исключённых групп"
            );
            ensure!(
                pools
                    .iter()
                    .all(|p| !p.is_empty() && p.len() <= 128 && !p.chars().any(char::is_control)),
                "Некорректное имя исключённой группы"
            );
        }
        Ok(())
    }
    pub fn flags(&self, hw: Hardware) -> Vec<String> {
        let mut result = if self.enabled {
            self.profile_for(hw).flags()
        } else {
            vec![]
        };
        for region in REGIONS {
            if let Some(address) = self.tunnels.get(region) {
                result.push(format!("-Droxy_address_override.{region}={address}"));
            }
        }
        result
    }
    pub fn export(&self, wrapper_only: bool, hw: Hardware) -> Result<Vec<u8>> {
        self.validate()?;
        if wrapper_only {
            Ok(serde_json::to_vec_pretty(&self.profile_for(hw))?)
        } else {
            Ok(serde_json::to_vec_pretty(&Portable {
                format: "exbo-launcher-optimization".into(),
                version: 2,
                settings: self.clone(),
            })?)
        }
    }
    pub fn import(bytes: &[u8], current: &Self, name: &str) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_PROFILE as usize,
            "Профиль больше 256 КиБ"
        );
        let shape: Value =
            serde_json::from_slice(bytes).context("Файл не является JSON-профилем")?;
        let imported = if shape.get("format").is_some() {
            let portable: Portable =
                serde_json::from_slice(bytes).context("Некорректный формат полного профиля")?;
            ensure!(
                portable.format == "exbo-launcher-optimization"
                    && matches!(portable.version, 1 | 2),
                "Версия профиля не поддерживается"
            );
            portable.settings
        } else {
            let profile: JvmProfile =
                serde_json::from_slice(bytes).context("Неизвестные поля или типы в JVM-профиле")?;
            Self {
                name: name.chars().take(80).collect(),
                profile: Some(profile),
                ..current.clone()
            }
        };
        imported.validate()?;
        Ok(imported)
    }
    pub fn read_import(path: &Path, current: &Self) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_PROFILE + 1)
            .read_to_end(&mut bytes)?;
        Self::import(
            &bytes,
            current,
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported"),
        )
    }
}

fn identity(arg: &str) -> &str {
    if arg.starts_with("-Xmx") || arg.starts_with("-Xms") || arg.starts_with("-Xmn") {
        &arg[..4]
    } else if arg.starts_with("-XX:+") || arg.starts_with("-XX:-") {
        &arg[5..]
    } else if let Some(rest) = arg.strip_prefix("-XX:") {
        rest.split('=').next().unwrap_or(rest)
    } else if arg.starts_with("-D") {
        arg.split('=').next().unwrap_or(arg)
    } else {
        arg
    }
}

pub fn jvm_target_index(args: &[String]) -> Result<usize> {
    let executable = args.first().context("Нет команды запуска")?;
    let filename = executable.rsplit(['\\', '/']).next().unwrap_or("");
    if !filename.eq_ignore_ascii_case("ExensLauncher.exe") {
        return Ok(0);
    }
    ensure!(
        args.len() >= 4
            && matches!(args[1].as_str(), "-platform" | "--platform")
            && !args[2].is_empty()
            && !args[2].starts_with('-'),
        "Неизвестный формат команды EXENS: ожидается -platform <платформа> <Java-программа>"
    );
    let child = args[3].rsplit(['\\', '/']).next().unwrap_or("");
    ensure!(
        [
            "java.exe",
            "javaw.exe",
            "stalzone.exe",
            "stalzonew.exe",
            "stalcraft.exe",
            "stalcraftw.exe"
        ]
        .iter()
        .any(|name| child.eq_ignore_ascii_case(name)),
        "В команде EXENS не найдена поддерживаемая Java-программа игры"
    );
    Ok(3)
}

pub fn inject(args: &[String], flags: &[String], full_profile: bool) -> Result<Vec<String>> {
    ensure!(!args.is_empty(), "Нет команды Java");
    if flags.is_empty() {
        return Ok(args.to_vec());
    }
    let target = jvm_target_index(args)?;
    if target != 0 {
        let mut result = args[..target].to_vec();
        result.extend(inject(&args[target..], flags, full_profile)?);
        return Ok(result);
    }
    let (mut boundary, mut i) = (args.len(), 1);
    while i < args.len() {
        let a = args[i].as_str();
        if matches!(a, "-jar" | "-m" | "--module") || !a.starts_with('-') {
            boundary = i;
            break;
        }
        if matches!(
            a,
            "-cp"
                | "-classpath"
                | "--class-path"
                | "-p"
                | "--module-path"
                | "--add-reads"
                | "--module-version"
                | "--source"
                | "--add-opens"
                | "--add-exports"
                | "--add-modules"
                | "--limit-modules"
                | "--patch-module"
                | "--upgrade-module-path"
                | "--enable-native-access"
        ) {
            ensure!(i + 1 < args.len(), "В команде Java нет значения для {a}");
            i += 2;
        } else {
            i += 1;
        }
    }
    let mut managed: HashSet<String> = flags.iter().map(|f| identity(f).to_owned()).collect();
    if full_profile {
        let mut every = aikar();
        every.pre_touch = true;
        every.use_string_deduplication = true;
        every.use_large_pages = true;
        for flag in every.flags() {
            managed.insert(identity(&flag).into());
        }
        for name in [
            "-Xmn",
            "LargePageSizeInBytes",
            "UseCounterDecay",
            "DontCompileHugeMethods",
            "UseSerialGC",
            "UseParallelGC",
            "UseParallelOldGC",
            "UseConcMarkSweepGC",
            "UseParNewGC",
            "UseZGC",
            "UseShenandoahGC",
            "UseEpsilonGC",
            "UseNUMA",
            "UseCompressedOops",
            "PerfDisableSharedMem",
            "PrintCommandLineFlags",
        ] {
            managed.insert(name.into());
        }
    }
    let mut result = vec![args[0].clone()];
    i = 1;
    while i < boundary {
        let a = &args[i];
        if matches!(
            a.as_str(),
            "-cp"
                | "-classpath"
                | "--class-path"
                | "-p"
                | "--module-path"
                | "--add-reads"
                | "--module-version"
                | "--source"
                | "--add-opens"
                | "--add-exports"
                | "--add-modules"
                | "--limit-modules"
                | "--patch-module"
                | "--upgrade-module-path"
                | "--enable-native-access"
        ) {
            result.extend_from_slice(&args[i..i + 2]);
            i += 2;
            continue;
        }
        if !managed.contains(identity(a)) {
            result.push(a.clone());
        }
        i += 1;
    }
    result.extend_from_slice(flags);
    result.extend_from_slice(&args[boundary..]);
    Ok(result)
}

pub fn java_major(text: &str) -> Option<u32> {
    let value = text.split("version \"").nth(1)?.split('"').next()?;
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    if major == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(major)
    }
}

pub fn validate_interceptor(exe: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::{Win32::System::Registry::*, core::PCWSTR};
        if let Some(name) = exe.file_name().and_then(|s| s.to_str()) {
            let key: Vec<u16> = format!(
                r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\{name}"
            )
            .encode_utf16()
            .chain(Some(0))
            .collect();
            let name: Vec<u16> = "Debugger".encode_utf16().chain(Some(0)).collect();
            let mut size = 0;
            let status = unsafe {
                RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(name.as_ptr()),
                    RRF_RT_ANY | RRF_NOEXPAND | RRF_SUBKEY_WOW6464KEY,
                    None,
                    None,
                    Some(&mut size),
                )
            };
            let absent = status == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND
                || status == windows::Win32::Foundation::ERROR_PATH_NOT_FOUND;
            if !absent {
                status
                    .ok()
                    .context("Не удалось проверить IFEO-перехватчик игры")?;
            }
            let hooked = !absent && size > 2;
            ensure!(
                !hooked,
                "Обнаружен внешний перехватчик запуска игры (IFEO). Отключите его через исходную утилиту. Если старый service.exe/wrapper.exe удалён, сохраните скрипт восстановления в «Оптимизация → Справка» и сначала просмотрите найденные записи"
            );
        }
    }
    Ok(())
}

pub fn validate_runtime(exe: &Path) -> Result<()> {
    let probe = exe
        .parent()
        .map(|p| p.join("java.exe"))
        .filter(|p| p.is_file())
        .context("Не найдена java.exe рядом с Java-программой игры; восстановите файлы клиента")?;
    let output = crate::platform::hidden(std::process::Command::new(probe).arg("-version"))
        .output()
        .context("Не удалось проверить версию Java игры")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    ensure!(
        output.status.success() && java_major(&text) == Some(9),
        "Профиль оптимизации рассчитан на Java 9 игры. Выключите оптимизацию для другой версии Java"
    );
    ensure!(
        text.contains("64-Bit") || text.contains("64-bit"),
        "Оптимизация требует 64-битную Java игры"
    );
    Ok(())
}
