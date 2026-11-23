use soroban_sdk::{
    contracttype, symbol_short,
    testutils::Address as _,
    xdr::{ScBytes, ScVal},
    Address, Env, Map, Symbol, Val,
};
use soroban_testkit_assert::{
    assert_snapshot_matches, snapshot_json, snapshot_path, SnapshotOptions,
};
use std::{
    any::Any,
    env as std_env, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

static ENV_LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
struct SnapshotState {
    updated_at: u64,
    who: Address,
    count: u32,
}

struct TempSnapshotDir {
    path: PathBuf,
    previous_dir: Option<std::ffi::OsString>,
    previous_update: Option<std::ffi::OsString>,
}

impl TempSnapshotDir {
    fn new(label: &str) -> Self {
        let path = std_env::temp_dir().join(format!(
            "testkit-snapshot-{}-{label}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temporary snapshot directory");
        let previous_dir = std_env::var_os("SNAPSHOT_DIR");
        let previous_update = std_env::var_os("UPDATE_SNAPSHOTS");
        std_env::set_var("SNAPSHOT_DIR", &path);
        std_env::remove_var("UPDATE_SNAPSHOTS");
        Self {
            path,
            previous_dir,
            previous_update,
        }
    }
}

impl Drop for TempSnapshotDir {
    fn drop(&mut self) {
        match &self.previous_dir {
            Some(value) => std_env::set_var("SNAPSHOT_DIR", value),
            None => std_env::remove_var("SNAPSHOT_DIR"),
        }
        match &self.previous_update {
            Some(value) => std_env::set_var("UPDATE_SNAPSHOTS", value),
            None => std_env::remove_var("UPDATE_SNAPSHOTS"),
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else {
        String::new()
    }
}

#[test]
fn auto_creation_exact_match_and_update() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("auto");

    assert_snapshot_matches("created", r#"{"count": 1}"#);
    let path = snapshot_path("created");
    assert!(path.is_file());
    let stored = fs::read_to_string(&path).expect("read created snapshot");
    assert!(stored.contains("\"count\": 1"), "{stored}");

    assert_snapshot_matches("created", r#"{"count": 1}"#);

    std_env::set_var("UPDATE_SNAPSHOTS", "1");
    assert_snapshot_matches("created", r#"{"count": 2}"#);
    let updated = fs::read_to_string(&path).expect("read updated snapshot");
    assert!(updated.contains("\"count\": 2"), "{updated}");
    assert!(!updated.contains("\"count\": 1"), "{updated}");
}

#[test]
fn mismatch_reports_colored_unified_diff() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("diff");
    let path = snapshot_path("drift");
    fs::write(&path, "{\n  \"count\": 1,\n  \"label\": \"old\"\n}\n")
        .expect("write stored snapshot");

    let result = std::panic::catch_unwind(|| {
        assert_snapshot_matches("drift", r#"{"count": 2, "label": "new"}"#);
    });
    let message = panic_message(result.expect_err("drift must fail"));
    assert!(
        message.contains("snapshot assertion failed for \"drift\""),
        "{message}"
    );
    assert!(message.contains("@@ -1,4 +1,4 @@"), "{message}");
    assert!(
        message.contains("\x1b[31m-  \"count\": 1,\x1b[0m"),
        "{message}"
    );
    assert!(
        message.contains("\x1b[32m+  \"count\": 2,\x1b[0m"),
        "{message}"
    );
    assert!(
        message.contains("\x1b[36m--- expected (stored):"),
        "{message}"
    );
    assert!(message.contains("UPDATE_SNAPSHOTS=1"), "{message}");
}

#[test]
fn renamed_and_omitted_fields_are_detected() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("fields");
    fs::write(
        snapshot_path("schema"),
        r#"{"amount": 5, "owner": "alice", "nonce": 1}"#,
    )
    .expect("write stored snapshot");

    let result = std::panic::catch_unwind(|| {
        assert_snapshot_matches("schema", r#"{"total": 5, "owner": "bob"}"#);
    });
    let message = panic_message(result.expect_err("field changes must fail"));
    assert!(message.contains("-  \"amount\": 5"), "{message}");
    assert!(message.contains("+  \"total\": 5"), "{message}");
    assert!(message.contains("-  \"nonce\": 1"), "{message}");
    assert!(message.contains("-  \"owner\": \"alice\""), "{message}");
    assert!(message.contains("+  \"owner\": \"bob\""), "{message}");
}

#[test]
fn options_normalize_addresses_and_ignore_fields() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("options");
    let env = Env::default();
    let options = SnapshotOptions::new()
        .normalize_addresses(true)
        .ignore_field("updated_at");

    let first = SnapshotState {
        updated_at: 100,
        who: Address::generate(&env),
        count: 3,
    };
    assert_snapshot_matches!("fuzzy", first, env, options.clone());
    let stored = fs::read_to_string(snapshot_path("fuzzy")).expect("read fuzzy snapshot");
    assert!(stored.contains("~address0"), "{stored}");
    assert!(!stored.contains("updated_at"), "{stored}");

    let changed = SnapshotState {
        updated_at: 999,
        who: Address::generate(&env),
        count: 3,
    };
    assert_snapshot_matches!("fuzzy", changed, env, options);
}

#[test]
fn two_arg_macro_uses_a_default_environment() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("two_arg");
    assert_snapshot_matches!("two_arg", 7u32);
    assert_snapshot_matches!("two_arg", 7u32);
    assert!(fs::read_to_string(snapshot_path("two_arg"))
        .expect("read snapshot")
        .contains('7'));
}

#[test]
fn explicit_env_arm_uses_the_given_environment() {
    let _guard = lock_env();
    let _dir = TempSnapshotDir::new("explicit");
    let env = Env::default();
    assert_snapshot_matches!("explicit_env", 42u32, env);
    assert_snapshot_matches!("explicit_env", 42u32, env);
    assert!(fs::read_to_string(snapshot_path("explicit_env"))
        .expect("read snapshot")
        .contains("42"));
}

#[test]
fn canonical_rendering_is_deterministic_for_vals_maps_and_contracttypes() {
    let _guard = lock_env();
    let env = Env::default();

    let mut first = Map::<Symbol, u32>::new(&env);
    first.set(symbol_short!("b"), 1);
    first.set(symbol_short!("a"), 2);
    let mut second = Map::<Symbol, u32>::new(&env);
    second.set(symbol_short!("a"), 2);
    second.set(symbol_short!("b"), 1);
    let rendered = snapshot_json(&env, &first);
    assert_eq!(rendered, snapshot_json(&env, &second));
    assert!(rendered.find("\"a\"").unwrap() < rendered.find("\"b\"").unwrap());

    assert_eq!(snapshot_json(&env, &Val::from(7u32)), "7");
    assert_eq!(snapshot_json(&env, &ScVal::U32(9)), "9");
    assert_eq!(
        snapshot_json(
            &env,
            &ScVal::Bytes(ScBytes::try_from(vec![0x0a, 0xff]).unwrap())
        ),
        "\"0x0aff\""
    );

    let state = SnapshotState {
        updated_at: 12,
        who: Address::generate(&env),
        count: 3,
    };
    let rendered = snapshot_json(&env, &state);
    assert!(rendered.contains("\"count\": 3"), "{rendered}");
    assert!(rendered.contains("\"updated_at\": 12"), "{rendered}");
    assert!(rendered.contains("\"who\": {"), "{rendered}");
}
