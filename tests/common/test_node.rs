use std::fs;
use std::path::{Path, PathBuf};

const TEST_PIN: &str = "767b5afd388c7981bcdf6f5b5c80159607e07e5b";

pub struct TestNodeFixtures {
    pub config_path: PathBuf,
    pub circuits_path: PathBuf,
    pub sequencer_observation_path: PathBuf,
    pub python_path: PathBuf,
}

pub fn setup_test_node_project(project_root: &Path) -> TestNodeFixtures {
    let lez_path = project_root.join("lez");
    fs::create_dir_all(&lez_path).expect("create lez path");
    let python_path = python3_path();
    let sequencer_observation_path = project_root.join("test-node-sequencer-observation.json");
    let config_path =
        write_test_node_sequencer_stub(&lez_path, &sequencer_observation_path, &python_path);
    let circuits_path = write_circuits_stub(project_root);
    write_scaffold_toml(project_root, &lez_path);
    TestNodeFixtures {
        config_path,
        circuits_path,
        sequencer_observation_path,
        python_path,
    }
}

pub fn test_node_observed_config_path(observation_path: &Path) -> PathBuf {
    let observation =
        fs::read_to_string(observation_path).expect("read test-node sequencer observation");
    let observation: serde_json::Value =
        serde_json::from_str(&observation).expect("parse test-node sequencer observation");
    let config_path = observation["config_path"]
        .as_str()
        .unwrap_or_else(|| panic!("test-node sequencer observation missing config_path"));
    PathBuf::from(config_path)
}

pub fn assert_test_node_launched_with_config(observation_path: &Path, expected_config_path: &Path) {
    let actual = test_node_observed_config_path(observation_path);
    let actual = actual.canonicalize().unwrap_or(actual);
    let expected = expected_config_path
        .canonicalize()
        .unwrap_or_else(|_| expected_config_path.to_path_buf());
    assert_eq!(
        actual, expected,
        "expected test-node sequencer to receive runtime config path"
    );
}

fn write_test_node_sequencer_stub(
    lez_path: &Path,
    observation_path: &Path,
    python_path: &Path,
) -> PathBuf {
    let sequencer_bin = lez_path.join("target/release/sequencer_service");
    let config_path = lez_path.join("sequencer/service/configs/debug/sequencer_config.json");
    fs::create_dir_all(sequencer_bin.parent().expect("parent")).expect("create sequencer dir");
    fs::create_dir_all(config_path.parent().expect("parent")).expect("create config dir");
    fs::write(
        &config_path,
        r#"{"home": ".", "genesis_id": 1, "block_create_timeout": "2s", "retry_pending_blocks_timeout": "3s"}"#,
    )
    .expect("write sequencer config");
    fs::write(
        &sequencer_bin,
        format!(
            r#"#!/bin/sh
exec {} - {} "$@" <<'PY'
import http.server
import json
import os
import socketserver
import sys

port = None
observation_path = sys.argv[1]
args = sys.argv[2:]
config_path = None

for index, arg in enumerate(args):
    if arg == "--port" and index + 1 < len(args):
        port = int(args[index + 1])
    if config_path is None and os.path.isfile(arg):
        try:
            with open(arg, encoding="utf-8") as candidate:
                json.load(candidate)
            config_path = arg
        except Exception:
            pass

if port is None:
    raise SystemExit("missing --port")
if config_path is None:
    raise SystemExit("missing config path")

with open(observation_path, "w", encoding="utf-8") as out:
    json.dump({{"args": args, "config_path": config_path, "port": port}}, out)

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        if length:
            self.rfile.read(length)
        body = b'{{"jsonrpc":"2.0","result":1,"id":1}}'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        pass

socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", port), Handler) as server:
    server.serve_forever()
PY
"#,
            sh_quote(python_path),
            sh_quote(observation_path),
        ),
    )
    .expect("write fake test-node sequencer");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&sequencer_bin)
            .expect("metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&sequencer_bin, perms).expect("chmod");
    }

    config_path
}

fn write_scaffold_toml(project_root: &Path, lez_path: &Path) {
    let spel_path = project_root.join("spel");
    let content = format!(
        "[scaffold]\nversion = \"0.2.0\"\ncache_root = \"{}\"\n\n[repos.lez]\nsource = \"https://github.com/logos-blockchain/logos-execution-zone.git\"\npin = \"{}\"\npath = \"{}\"\n\n[repos.spel]\nsource = \"https://github.com/logos-co/spel.git\"\npin = \"{}\"\npath = \"{}\"\n\n[wallet]\nhome_dir = \".scaffold/wallet\"\n",
        project_root.join("cache").display(),
        TEST_PIN,
        lez_path.display(),
        TEST_PIN,
        spel_path.display(),
    );
    fs::write(project_root.join("scaffold.toml"), content).expect("write scaffold.toml");
}

fn write_circuits_stub(project_root: &Path) -> PathBuf {
    let circuits_path = project_root.join("circuits");
    let sentinel = circuits_path.join("pol/verification_key.json");
    fs::create_dir_all(sentinel.parent().expect("parent")).expect("create circuits dir");
    fs::write(&sentinel, "{}").expect("write circuits sentinel");
    circuits_path
}

fn python3_path() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH must be set for test-node fixtures");
    std::env::split_paths(&path)
        .map(|dir| dir.join("python3"))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
        .expect("python3 must be available for test-node fixtures")
}

fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}
