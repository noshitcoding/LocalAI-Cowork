//! Synthetic process for document-probe lifecycle regression tests. Never packaged.
use cowork_toolbox::{Config, Error, Toolbox};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--attempt") {
        let result = Toolbox::open(Config {
            root: PathBuf::from(&args[1]),
            programs: BTreeMap::new(),
            environment: BTreeMap::new(),
            github_token: None,
        });
        match result {
            Ok(owner) => {
                drop(owner);
                println!("opened");
            }
            Err(Error::RootInUse) => {
                println!("busy");
                std::process::exit(2);
            }
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }
    let control = PathBuf::from(
        std::env::var_os("COWORK_PROBE_FIXTURE_CONTROL").ok_or("missing fixture control")?,
    );
    let phase = std::env::var("COWORK_PROBE_FIXTURE_PHASE")?;
    let is_office = args.first().is_some_and(|arg| arg == "--headless");
    if !is_office && args.iter().any(|arg| arg.ends_with("ocr_probe.py")) {
        println!("{{}}");
        return Ok(());
    }
    if !is_office && phase == "office" {
        println!(
            "{{\"format\":\"cowork-document-probe/1\",\"ok\":true,\"engine_version\":\"synthetic-probe\"}}"
        );
        return Ok(());
    }
    let (input, profile) = if is_office {
        let input = PathBuf::from(args.last().ok_or("missing Office input")?);
        let uri = args
            .iter()
            .find_map(|arg| arg.strip_prefix("-env:UserInstallation="))
            .ok_or("missing Office profile")?;
        (
            input,
            Some(
                url::Url::parse(uri)?
                    .to_file_path()
                    .map_err(|_| "invalid Office URI")?,
            ),
        )
    } else {
        let root = if std::env::var_os("COWORK_PROBE_FIXTURE_USE_ADAPTER_ROOT").as_deref()
            == Some(std::ffi::OsStr::new("1"))
        {
            // The native factory chooses its slot after Config.environment is fixed.
            // Derive only the synthetic probe's stage root from the actual adapter
            // argument: slot/adapters/documents-digest/document_engine.py.
            let adapter = args
                .iter()
                .map(std::path::Path::new)
                .find(|path| {
                    path.file_name()
                        .is_some_and(|name| name == "document_engine.py")
                })
                .ok_or("missing document adapter argument")?;
            adapter
                .parent()
                .and_then(std::path::Path::parent)
                .and_then(std::path::Path::parent)
                .ok_or("invalid document adapter ancestry")?
                .to_path_buf()
        } else {
            PathBuf::from(
                std::env::var_os("COWORK_PROBE_FIXTURE_ROOT").ok_or("missing fixture root")?,
            )
        };
        let stage = root
            .join("document-stages")
            .join("7b3f53de-4c54-44c9-8b08-1dc61229e1e7");
        fs::create_dir(&stage)?;
        let input = stage.join("original.txt");
        fs::write(&input, b"original probe input")?;
        (input, None)
    };
    let original = fs::read(&input)?;
    fs::write(
        control.join("started.json"),
        serde_json::to_vec(
            &serde_json::json!({"pid":std::process::id(), "input":input, "profile":profile}),
        )?,
    )?;
    let until = Instant::now() + Duration::from_secs(20);
    while !control.join("release").exists() {
        if Instant::now() >= until {
            return Err("fixture release timed out".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let original_input_preserved = fs::read(&input).is_ok_and(|bytes| bytes == original);
    let original_profile_preserved = profile.as_ref().is_none_or(|path| path.is_dir());
    let exit_code: i32 = std::env::var("COWORK_PROBE_FIXTURE_EXIT")?.parse()?;
    if is_office && original_input_preserved && original_profile_preserved && exit_code == 0 {
        fs::write(
            input.with_extension("pdf"),
            b"%PDF-synthetic lifecycle fixture",
        )?;
    }
    fs::write(
        control.join("finished.json"),
        serde_json::to_vec(
            &serde_json::json!({"original_input_preserved":original_input_preserved,"original_profile_preserved":original_profile_preserved,"requested_exit":exit_code}),
        )?,
    )?;
    // A failing synthetic Python probe avoids starting another probe phase.
    std::process::exit(if is_office { exit_code } else { 7 });
}
