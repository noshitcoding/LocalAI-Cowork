use super::*;
use std::fs;

pub fn run(args: &[String]) {
    match args[0].as_str() {
        "isolation-owner" => {
            use cowork_executor::windows_isolation::{Capsule, Limits};
            use sha2::{Digest, Sha256};
            let mut capsule = Capsule::create(
                Path::new(&args[1]),
                Limits {
                    memory_bytes: 256 * 1024 * 1024,
                    processes: 4,
                    cpu_millis: 1000,
                },
            )
            .unwrap();
            let bytes = fs::read(&args[2]).unwrap();
            capsule
                .stage_program(
                    "probe.exe",
                    &bytes,
                    &format!("{:x}", Sha256::digest(&bytes)),
                )
                .unwrap();
            println!(
                "{}",
                serde_json::json!({"root":capsule.directory(),"profile":capsule.profile_name(),"sid":capsule.sid().unwrap()})
            );
            std::io::stdout().flush().unwrap();
            let work = capsule.workspace().to_string_lossy().into_owned();
            let outcome = capsule
                .run(
                    "probe.exe",
                    vec!["heartbeat".into(), work],
                    Duration::from_secs(15),
                    &AtomicBool::new(false),
                )
                .unwrap();
            println!("{}", serde_json::to_string(&outcome).unwrap());
            capsule.close().unwrap();
        }
        _ => panic!("unknown isolation owner fixture"),
    }
}
