use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use portez::cli::Cli;
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    registry: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let a = root.path().join("a");
        let b = root.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        Self {
            registry: root.path().join("cfg").join("ports.toml"),
            _root: root,
            a,
            b,
        }
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_portez"))
            .args(args)
            .current_dir(dir)
            .env("PORTEZ_CONFIG", &self.registry)
            .output()
            .expect("portez should run")
    }

    fn stdout(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.run(dir, args);
        assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
        text(&out.stdout)
    }
}

fn canonical(dir: &Path) -> String {
    fs::canonicalize(dir)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn bare_invocation_registers_main_and_is_stable() {
    let fx = Fixture::new();
    let first = fx.run(&fx.a, &[]);
    assert!(first.status.success());
    assert_eq!(text(&first.stdout), "4001\n");
    assert!(
        text(&first.stderr).starts_with("portez: registered main → 4001 for "),
        "{}",
        text(&first.stderr)
    );

    let second = fx.run(&fx.a, &[]);
    assert_eq!(text(&second.stdout), "4001\n");
    assert_eq!(text(&second.stderr), "", "no notice on a repeat lookup");
}

#[test]
fn names_and_directories_get_distinct_ports() {
    let fx = Fixture::new();
    assert_eq!(fx.stdout(&fx.a, &["main"]), "4001\n");
    assert_eq!(fx.stdout(&fx.a, &["debug"]), "4002\n");
    assert_eq!(fx.stdout(&fx.b, &["-q"]), "4003\n");
    assert_eq!(fx.stdout(&fx.a, &["get", "debug"]), "4002\n");
    assert_eq!(
        fx.stdout(&fx.b, &["-C", fx.a.to_str().unwrap(), "main"]),
        "4001\n"
    );

    let toml = fs::read_to_string(&fx.registry).unwrap();
    assert!(toml.contains("main = 4001"), "{toml}");
    assert!(toml.contains("debug = 4002"), "{toml}");
    assert!(toml.contains("main = 4003"), "{toml}");
}

#[test]
fn quiet_suppresses_the_notice() {
    let fx = Fixture::new();
    let out = fx.run(&fx.a, &["--quiet", "web"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "4001\n");
    assert_eq!(text(&out.stderr), "");
}

#[test]
fn list_and_ls_show_assignments_sorted_by_port() {
    let fx = Fixture::new();
    fx.stdout(&fx.b, &["-q", "main"]);
    fx.stdout(&fx.a, &["-q", "debug"]);
    fx.stdout(&fx.a, &["-q", "main"]);

    let a = canonical(&fx.a);
    let b = canonical(&fx.b);
    let expected =
        format!("PORT   NAME   DIR\n4001   main   {b}\n4002   debug  {a}\n4003   main   {a}\n");
    assert_eq!(fx.stdout(&fx.a, &["list"]), expected);
    assert_eq!(fx.stdout(&fx.a, &["ls"]), expected);

    let json: serde_json::Value =
        serde_json::from_str(&fx.stdout(&fx.a, &["ls", "--json"])).unwrap();
    assert_eq!(json[0]["port"], 4001);
    assert_eq!(json[0]["dir"], b);
    assert_eq!(json[2]["name"], "main");
}

#[test]
fn empty_list_prints_nothing_on_stdout() {
    let fx = Fixture::new();
    let out = fx.run(&fx.a, &["ls"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "");
}

#[test]
fn existing_does_not_register() {
    let fx = Fixture::new();
    let out = fx.run(&fx.a, &["get", "--existing"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("no port registered"));
    assert!(!fx.registry.exists());

    fx.stdout(&fx.a, &["-q"]);
    assert_eq!(fx.stdout(&fx.a, &["get", "--existing"]), "4001\n");
}

#[test]
fn rm_forgets_one_name_or_the_whole_directory() {
    let fx = Fixture::new();
    fx.stdout(&fx.a, &["-q"]);
    fx.stdout(&fx.a, &["-q", "debug"]);
    fx.stdout(&fx.b, &["-q"]);

    let out = fx.run(&fx.a, &["rm", "debug"]);
    assert!(out.status.success());
    assert!(text(&out.stderr).contains("removed debug → 4002"));
    assert_eq!(
        fx.stdout(&fx.b, &["-q", "extra"]),
        "4002\n",
        "gap is reused"
    );

    let out = fx.run(&fx.a, &["remove", "--all"]);
    assert!(out.status.success());
    let listed = fx.stdout(&fx.a, &["ls"]);
    assert!(!listed.contains(&canonical(&fx.a)), "{listed}");

    let out = fx.run(&fx.a, &["rm"]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn path_reports_the_registry_location() {
    let fx = Fixture::new();
    assert_eq!(
        fx.stdout(&fx.a, &["path"]),
        format!("{}\n", fx.registry.display())
    );
    let other = fx.registry.with_file_name("other.toml");
    assert_eq!(
        fx.stdout(&fx.a, &["--registry", other.to_str().unwrap(), "path"]),
        format!("{}\n", other.display())
    );
}

#[test]
fn settings_start_and_comments_survive() {
    let fx = Fixture::new();
    fs::create_dir_all(fx.registry.parent().unwrap()).unwrap();
    fs::write(&fx.registry, "# my ports\n[settings]\nstart = 8000\n").unwrap();
    assert_eq!(fx.stdout(&fx.a, &["-q"]), "8000\n");
    let toml = fs::read_to_string(&fx.registry).unwrap();
    assert!(toml.starts_with("# my ports\n"), "{toml}");
}

#[test]
fn invalid_input_uses_distinct_exit_codes() {
    let fx = Fixture::new();
    let bad_name = fx.run(&fx.a, &["bad name"]);
    assert_eq!(bad_name.status.code(), Some(1));
    assert!(text(&bad_name.stderr).contains("invalid name"));

    let bad_flag = fx.run(&fx.a, &["--bogus"]);
    assert_eq!(bad_flag.status.code(), Some(2));

    let missing_dir = fx.run(&fx.a, &["-C", "/definitely/not/here"]);
    assert_eq!(missing_dir.status.code(), Some(1));
    assert!(text(&missing_dir.stderr).contains("cannot resolve directory"));

    fs::create_dir_all(fx.registry.parent().unwrap()).unwrap();
    fs::write(
        &fx.registry,
        "[ports.\"/x\"]\nmain = 4001\n[ports.\"/y\"]\nmain = 4001\n",
    )
    .unwrap();
    let corrupt = fx.run(&fx.a, &["ls"]);
    assert_eq!(corrupt.status.code(), Some(1));
    assert!(text(&corrupt.stderr).contains("assigned more than once"));
}

#[test]
fn concurrent_registrations_never_collide() {
    let fx = Fixture::new();
    let dirs: Vec<PathBuf> = (0..16)
        .map(|i| {
            let dir = fx.a.join(format!("p{i}"));
            fs::create_dir_all(&dir).unwrap();
            dir
        })
        .collect();
    let children: Vec<_> = dirs
        .iter()
        .map(|dir| {
            Command::new(env!("CARGO_BIN_EXE_portez"))
                .arg("-q")
                .current_dir(dir)
                .env("PORTEZ_CONFIG", &fx.registry)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut ports: Vec<u16> = children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success());
            text(&out.stdout).trim().parse().unwrap()
        })
        .collect();
    ports.sort_unstable();
    assert_eq!(ports, (4001..=4016).collect::<Vec<u16>>());
}

#[test]
fn usage_spec_and_completions_are_available() {
    let kdl = Cli::to_kdl();
    let parsed: usage_parser::Spec = kdl.parse().expect("Usage KDL should round-trip");
    assert_eq!(parsed.bin, "portez");
    for name in ["get", "list", "rm", "path", "completion"] {
        assert!(parsed.cmd.subcommands.contains_key(name), "{name}");
    }

    let fx = Fixture::new();
    let spec = fx.stdout(&fx.a, &["__usage_spec__"]);
    assert!(spec.starts_with("name portez\nbin portez\n"), "{spec}");
    for shell in ["bash", "zsh", "fish", "nushell", "powershell", "elvish"] {
        let script = fx.stdout(&fx.a, &["completion", shell]);
        assert!(script.contains("portez"), "{shell}: {script}");
    }
}
