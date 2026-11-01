//! Secret metadata and executor disclosure policy. Values stay in the caller's environment.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::sync::{LazyLock, RwLock};

use super::ProcessingPolicy;
use crate::config::{Config, ModelsCfg};

#[derive(Debug, Clone)]
pub struct SecretVariable {
    pub name: String,
    pub policy: ProcessingPolicy,
    pub child_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SecretRegistry {
    variables: BTreeMap<String, SecretVariable>,
}

// No Debug: the assembled environment contains credential values.
pub struct ExecutorEnvironment {
    pub variables: Vec<(OsString, OsString)>,
    pub missing: Vec<String>,
}

fn registry_key(name: &str) -> String {
    if cfg!(windows) { name.to_ascii_uppercase() } else { name.to_string() }
}

pub fn env_name_matches(name: &OsStr, expected: &str) -> bool {
    if cfg!(windows) { name.to_str().is_some_and(|name| name.eq_ignore_ascii_case(expected)) } else { name == expected }
}

impl Default for SecretRegistry {
    fn default() -> Self {
        let mut registry = Self { variables: BTreeMap::new() };
        for name in
            ["TELEGRAM_BOT_TOKEN", "TELEGRAM_PROXY", "ONEBOT_TOKEN", "BAILIAN_API_KEY", "OWNER_TELEGRAM_ID", "OWNER_QQ_ID", "MONGO_URL"]
        {
            registry.local_only(name);
        }
        for (name, child_name) in [("AGLI_DSH_DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY"), ("AGLI_DSH_BAILIAN_API_KEY", "DSH_BAILIAN_API_KEY")] {
            registry.variables.insert(
                registry_key(name),
                SecretVariable {
                    name: name.into(),
                    policy: ProcessingPolicy::Providers { names: vec!["dsh".into()] },
                    child_name: Some(child_name.into()),
                },
            );
        }
        registry
    }
}

impl SecretRegistry {
    fn local_only(&mut self, name: &str) {
        self.variables
            .insert(registry_key(name), SecretVariable { name: name.into(), policy: ProcessingPolicy::LocalOnly, child_name: None });
    }

    fn protect_models(&mut self, models: &ModelsCfg) {
        for name in models.providers.values().filter_map(|provider| provider.api_key_env.as_deref()) {
            // Configured AGLI credentials take precedence over any delegate grant.
            self.local_only(name);
        }
    }

    fn protect_config(&mut self, cfg: &Config) {
        self.protect_models(&cfg.models);
        for name in [
            Some(cfg.acl.owner.telegram_user_id_env.as_str()),
            cfg.acl.owner.qq_user_id_env.as_deref(),
            cfg.channels.telegram.proxy_env.as_deref(),
            Some(cfg.channels.qq.token_env.as_str()),
            Some(cfg.channels.qq.url_env.as_str()),
            Some(cfg.runtime.mongo.url_env.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            self.local_only(name);
        }
    }

    pub fn assemble(
        &self,
        executor: &str,
        runtime_allowlist: &[&str],
        parent: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> ExecutorEnvironment {
        let parent: Vec<_> = parent.into_iter().collect();
        let mut variables = Vec::new();
        let mut missing = Vec::new();
        for (name, value) in &parent {
            let registered = name.to_str().is_some_and(|name| self.variables.contains_key(&registry_key(name)));
            if !registered && !looks_secret(name, value) && runtime_allowlist.iter().any(|allowed| env_name_matches(name, allowed)) {
                variables.push((name.clone(), value.clone()));
            }
        }
        for secret in self.variables.values().filter(|secret| secret.policy.allows_executor(executor)) {
            match parent.iter().find(|(name, _)| env_name_matches(name, &secret.name)).filter(|(_, value)| !value.is_empty()) {
                Some((_, value)) => variables.push((secret.child_name.as_deref().unwrap_or(&secret.name).into(), value.clone())),
                None => missing.push(secret.name.clone()),
            }
        }
        ExecutorEnvironment { variables, missing }
    }
}

fn looks_secret(name: &OsStr, value: &OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "COOKIE", "CREDENTIAL", "CONNECTION_STRING", "CONNECTIONSTRING"]
        .iter().any(|part| name.contains(part))
        || name.starts_with("OWNER_")
        || name == "DSN"
        || ["_URL", "_URI", "_DSN"].iter().any(|suffix| name.ends_with(suffix))
        // Proxy URLs can hide credentials behind an otherwise harmless runtime name.
        || (name.ends_with("_PROXY") && value.to_str().is_none_or(|value| {
            let authority = value.split_once("://").map_or(value, |(_, rest)| rest).split(['/', '?', '#']).next().unwrap_or("");
            authority.contains('@')
        }))
}

static PROCESS_SECRETS: LazyLock<RwLock<SecretRegistry>> = LazyLock::new(|| RwLock::new(SecretRegistry::default()));

/// Called after successful config loading. Later loads can only add LocalOnly restrictions;
/// they cannot accidentally restore a delegate grant or unprotect an earlier credential.
pub fn register_config(cfg: &Config) {
    PROCESS_SECRETS.write().unwrap_or_else(|e| e.into_inner()).protect_config(cfg);
}

pub fn process_registry() -> SecretRegistry {
    PROCESS_SECRETS.read().unwrap_or_else(|e| e.into_inner()).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parent(entries: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        entries.iter().map(|(name, value)| ((*name).into(), (*value).into())).collect()
    }

    fn values(environment: ExecutorEnvironment) -> BTreeMap<OsString, OsString> {
        environment.variables.into_iter().collect()
    }

    #[test]
    fn defaults_keep_internal_secrets_local() {
        let registry = SecretRegistry::default();
        for name in
            ["TELEGRAM_BOT_TOKEN", "TELEGRAM_PROXY", "ONEBOT_TOKEN", "BAILIAN_API_KEY", "OWNER_TELEGRAM_ID", "OWNER_QQ_ID", "MONGO_URL"]
        {
            let secret = &registry.variables[&registry_key(name)];
            assert_eq!(secret.policy, ProcessingPolicy::LocalOnly);
            assert!(secret.child_name.is_none());
        }
        for (name, target) in [("AGLI_DSH_DEEPSEEK_API_KEY", "DEEPSEEK_API_KEY"), ("AGLI_DSH_BAILIAN_API_KEY", "DSH_BAILIAN_API_KEY")] {
            let secret = &registry.variables[&registry_key(name)];
            assert_eq!(secret.policy, ProcessingPolicy::Providers { names: vec!["dsh".into()] });
            assert_eq!(secret.child_name.as_deref(), Some(target));
        }
    }

    #[test]
    fn assembled_environments_follow_policy_before_runtime_allowlist() {
        let registry = SecretRegistry::default();
        let source = parent(&[
            ("PATH", "runtime"),
            ("HOME", "login"),
            ("BAILIAN_API_KEY", "bot"),
            ("TELEGRAM_PROXY", "http://user:password@localhost:7890"),
            ("OWNER_QQ_ID", "42"),
            ("MONGO_URL", "mongodb://user:password@localhost"),
            ("AGLI_DSH_DEEPSEEK_API_KEY", "search"),
            ("AGLI_DSH_BAILIAN_API_KEY", "model"),
            ("DEEPSEEK_API_KEY", "wrong-search"),
            ("DSH_BAILIAN_API_KEY", "wrong-model"),
        ]);
        // Deliberately pollute the runtime allowlist with every secret, including aliases.
        let allowlist: Vec<_> = source.iter().map(|(name, _)| name.to_str().unwrap()).collect();
        let expected = parent(&[("PATH", "runtime"), ("HOME", "login")]);
        for executor in ["codex", "dsh", "unknown"] {
            let result = registry.assemble(executor, &allowlist, source.clone());
            assert!(result.missing.is_empty());
            let mut expected: BTreeMap<_, _> = expected.clone().into_iter().collect();
            if executor == "dsh" {
                expected.extend(parent(&[("DEEPSEEK_API_KEY", "search"), ("DSH_BAILIAN_API_KEY", "model")]));
            }
            assert_eq!(values(result), expected);
        }
    }

    #[test]
    fn unregistered_secret_names_cannot_be_allowed_by_runtime_allowlist() {
        let names = [
            "monKEY",
            "new_ToKeN",
            "new_sEcReT",
            "new_PaSsWoRd",
            "new_cOoKiE",
            "new_cReDeNtIaL",
            "mongo_url",
            "mongo_uri",
            "DATABASE_URL",
            "POSTGRES_DSN",
            "DSN",
            "SQL_CONNECTION_STRING",
            "SQLCONNECTIONSTRING",
            "OWNER_FUTURE_ID",
        ];
        let source = names.iter().map(|name| ((*name).into(), "sensitive".into())).collect::<Vec<_>>();
        let registry = SecretRegistry::default();
        for executor in ["codex", "dsh"] {
            assert!(registry.assemble(executor, &names, source.clone()).variables.is_empty());
        }
    }

    #[test]
    fn runtime_proxies_with_credentials_are_not_public_runtime_values() {
        let registry = SecretRegistry::default();
        for value in ["http://user:password@localhost:7890", "socks5://user@localhost", "user:password@localhost:7890"] {
            assert!(registry.assemble("codex", &["https_proxy"], parent(&[("https_proxy", value)])).variables.is_empty());
        }
        let source = parent(&[("https_proxy", "http://localhost:7890"), ("NO_PROXY", "localhost,127.0.0.1")]);
        assert_eq!(values(registry.assemble("codex", &["https_proxy", "NO_PROXY"], source.clone())), source.into_iter().collect());
    }

    #[test]
    fn registered_secrets_use_processing_policy_and_optional_rename() {
        let mut registry = SecretRegistry::default();
        for (name, policy, child_name) in [
            ("SHARED_SECRET", ProcessingPolicy::Any, None),
            ("CODEX_SECRET", ProcessingPolicy::Providers { names: vec!["codex".into()] }, Some("RENAMED")),
            ("RUNTIME_PATH", ProcessingPolicy::LocalOnly, None),
        ] {
            registry
                .variables
                .insert(registry_key(name), SecretVariable { name: name.into(), policy, child_name: child_name.map(String::from) });
        }
        let source = parent(&[("SHARED_SECRET", "shared"), ("CODEX_SECRET", "private"), ("RUNTIME_PATH", "hidden")]);
        let codex = values(registry.assemble("codex", &["RUNTIME_PATH"], source.clone()));
        assert_eq!(codex, parent(&[("SHARED_SECRET", "shared"), ("RENAMED", "private")]).into_iter().collect());
        let dsh = values(registry.assemble("dsh", &["RUNTIME_PATH"], source));
        assert_eq!(dsh, parent(&[("SHARED_SECRET", "shared")]).into_iter().collect());
        let missing = registry.assemble("codex", &[], parent(&[("SHARED_SECRET", "")])).missing;
        assert_eq!(missing, ["CODEX_SECRET", "SHARED_SECRET"]);
    }

    #[test]
    fn provider_key_names_are_derived_from_models_config_and_override_grants() {
        let mut models: ModelsCfg = serde_yaml::from_str(include_str!("../../config/models.yaml")).unwrap();
        // No secret-looking name: only config registration can protect this runtime entry.
        models.providers.insert("future".into(), serde_yaml::from_str("kind: openai_compat\napi_key_env: CUSTOM_AUTH").unwrap());
        models
            .providers
            .insert("collision".into(), serde_yaml::from_str("kind: openai_compat\napi_key_env: AGLI_DSH_BAILIAN_API_KEY").unwrap());
        let mut registry = SecretRegistry::default();
        registry.protect_models(&models);
        for name in models.providers.values().filter_map(|provider| provider.api_key_env.as_deref()) {
            assert_eq!(registry.variables[&registry_key(name)].policy, ProcessingPolicy::LocalOnly);
        }
        let source = parent(&[("CUSTOM_AUTH", "future"), ("AGLI_DSH_BAILIAN_API_KEY", "now-local")]);
        assert!(registry.assemble("dsh", &["CUSTOM_AUTH"], source.clone()).variables.is_empty());
        models.providers.clear();
        registry.protect_models(&models);
        assert!(registry.assemble("dsh", &["CUSTOM_AUTH"], source).variables.is_empty());
    }

    #[test]
    fn successful_config_load_registers_all_declared_sensitive_names() {
        let cfg = Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let registry = process_registry();
        for name in cfg.models.providers.values().filter_map(|provider| provider.api_key_env.as_deref()) {
            assert_eq!(registry.variables[&registry_key(name)].policy, ProcessingPolicy::LocalOnly);
        }
        let mut cfg = cfg;
        cfg.acl.owner.telegram_user_id_env = "CUSTOM_OWNER".into();
        cfg.acl.owner.qq_user_id_env = Some("CUSTOM_PERSON".into());
        cfg.channels.telegram.proxy_env = Some("CUSTOM_PROXY".into());
        cfg.channels.qq.token_env = "CUSTOM_ACCESS".into();
        cfg.channels.qq.url_env = "CUSTOM_ENDPOINT".into();
        cfg.runtime.mongo.url_env = "CUSTOM_DATABASE".into();
        let mut registry = registry;
        registry.protect_config(&cfg);
        for name in ["CUSTOM_OWNER", "CUSTOM_PERSON", "CUSTOM_PROXY", "CUSTOM_ACCESS", "CUSTOM_ENDPOINT", "CUSTOM_DATABASE"] {
            assert_eq!(registry.variables[&registry_key(name)].policy, ProcessingPolicy::LocalOnly);
        }
    }
}
