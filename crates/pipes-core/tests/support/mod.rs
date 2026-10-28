use std::{
    fs,
    path::{Path, PathBuf},
};

use tempfile::{Builder, TempDir};

pub const MINIMAL: &str = "api: pipes/v1\n\
app: demo\n\
source: {path: source}\n\
version: {from: 'file:VERSION'}\n\
stages: [{id: build}]\n";

pub const FULL: &str = r#"api: pipes/v1
app: sample-app
name: Sample Application
source:
  path: source
  snapshot: git-commit
  require_clean: false
version:
  scheme: semver
  from: file:VERSION
execution:
  host:
    os: macos
    arch: arm64
  timeout: 1h 30m
prepare:
  - id: version
    exec: [tool-not-installed, "argument with spaces", ""]
    cwd: directory-not-created
    timeout: 10s
    env:
      BUILD_KIND: release
      EMPTY: ""
    secrets:
      TOKEN: env://PIPES_TEST_TOKEN
      ACCOUNT: keychain://service account
outputs:
  bundle:
    path: dist/My App.app
    format: directory
  binary:
    path: dist/My App
    format: file
stages:
  - id: tested
    checks:
      - id: unit
        exec: [cargo, test]
    require: [checks.passed, prepare.version.passed]
  - id: shipped
    require: [previous.passed, approval.local]
retention:
  mode: keep-all
  protect: [stage_heads, pins, active_runs]
"#;

pub struct Fixture {
    pub root: TempDir,
    pub manifest: PathBuf,
    pub source: PathBuf,
    pub state: PathBuf,
}

impl Fixture {
    pub fn new(yaml: &str) -> Self {
        let root = Builder::new()
            .prefix(".pipes-core-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let manifest = root.path().join("app.yaml");
        let source = root.path().join("source");
        let state = root.path().join("state");
        fs::create_dir(&source).unwrap();
        fs::write(&manifest, yaml).unwrap();
        Self {
            root,
            manifest,
            source,
            state,
        }
    }

    pub fn write(&self, yaml: &str) {
        fs::write(&self.manifest, yaml).unwrap();
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
}

pub fn canonical(path: &Path) -> String {
    fs::canonicalize(path).unwrap().to_str().unwrap().to_owned()
}
