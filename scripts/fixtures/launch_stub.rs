use std::{fs, process::Command};
use std::os::windows::process::CommandExt;
fn affinity() -> usize {
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn GetProcessAffinityMask(process: *mut std::ffi::c_void, mask: *mut usize, system: *mut usize) -> i32;
    }
    let (mut mask, mut system) = (0, 0);
    assert_ne!(unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut mask, &mut system) }, 0);
    mask
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let executable = std::env::current_exe().unwrap();
    let name = executable.file_name().unwrap().to_string_lossy();
    if name.eq_ignore_ascii_case("java.exe") && args == ["-version"] {
        fs::write(executable.parent().unwrap().join("fixture-probe.txt"), "java.exe -version").unwrap();
        eprintln!("openjdk version \"9.0.6.4\"\nOpenJDK 64-Bit Server VM");
        return;
    }
    if name.eq_ignore_ascii_case("ExensLauncher.exe") {
        fs::write("fixture-wrapper-cpu.txt", affinity().to_string()).unwrap();
        fs::write("fixture-wrapper-args.txt", args.join("\n")).unwrap();
        if args.len() < 3 || args[0] != "-platform" || args[1] != "exbo" {
            std::process::exit(17);
        }
        let status = Command::new(&args[2]).args(&args[3..]).creation_flags(0x08000000).status().unwrap();
        std::process::exit(status.code().unwrap_or(18));
    }
    assert!(name.eq_ignore_ascii_case("stalzone.exe"));
    fs::write("fixture-java-cpu.txt", affinity().to_string()).unwrap();
    fs::write("fixture-java-args.txt", args.join("\n")).unwrap();
}
