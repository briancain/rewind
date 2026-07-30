pub fn table(name: &str) -> String {
    let prefix = std::env::var("TABLE_PREFIX").unwrap_or_default();
    format!("{}{}", prefix, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `table()` reads the process-global TABLE_PREFIX env var, shared across test threads.
    // Serialize the env-mutating tests so they are parallel-safe (independent of `--test-threads`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn table_no_prefix() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("TABLE_PREFIX");
        assert_eq!(table("users"), "users");
    }

    #[test]
    fn table_with_prefix() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TABLE_PREFIX", "rewind-dev-");
        assert_eq!(table("users"), "rewind-dev-users");
        std::env::remove_var("TABLE_PREFIX");
    }
}
