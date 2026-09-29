//! The application core the desktop shell holds: storage, keys, the model
//! client and the knowledge base, plus helpers to reach the model set up for
//! each role.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};

use models::{Client, ModelProfile, Provider, ThinkingLevel};

use crate::enhanced::EnhancedOptions;
use crate::error::{Error, Result};
use crate::fix::Stored;
use crate::providers;
use crate::secrets::SecretStore;
use crate::settings::{RoleModel, Settings};
use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleName {
    Chat,
    Decision,
    Embedding,
    Rerank,
}

impl RoleName {
    pub fn label(self) -> &'static str {
        match self {
            RoleName::Chat => "大语言模型",
            RoleName::Decision => "决策模型",
            RoleName::Embedding => "向量模型",
            RoleName::Rerank => "重排模型",
        }
    }
}

/// A provider and model ready to call.
#[derive(Clone)]
pub struct Target {
    pub provider: Provider,
    pub model: String,
    pub profile: ModelProfile,
    pub thinking: Option<ThinkingLevel>,
}

pub struct Core {
    data_dir: PathBuf,
    store: Mutex<Store>,
    secrets: SecretStore,
    client: Client,
    pub(crate) proposals: Mutex<HashMap<String, Stored>>,
    /// Opened on first use.
    pub(crate) kb: Mutex<Option<kb::KnowledgeBase>>,
    /// Set while chunks are being embedded.
    pub(crate) embedding: AtomicBool,
    /// Service addresses and waits for enhanced parsing.
    pub(crate) enhanced: Mutex<EnhancedOptions>,
}

impl Core {
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let client = Client::new().map_err(|e| Error::Setup(e.to_string()))?;
        Self::with_parts(data_dir, client, SecretStore::new(data_dir))
    }

    /// A core with its own model client and key storage, e.g. for tests.
    pub fn with_parts(data_dir: &Path, client: Client, secrets: SecretStore) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            store: Mutex::new(Store::open(&data_dir.join("app.db"))?),
            secrets,
            client,
            proposals: Mutex::new(HashMap::new()),
            kb: Mutex::new(None),
            embedding: AtomicBool::new(false),
            enhanced: Mutex::new(EnhancedOptions::default()),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }

    pub fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn settings(&self) -> Result<Settings> {
        self.store().settings()
    }

    pub fn save_settings(&self, settings: Settings) -> Result<Settings> {
        let settings = settings.normalized();
        self.store().save_settings(&settings)?;
        Ok(settings)
    }

    fn role_model(settings: &Settings, role: RoleName) -> &RoleModel {
        match role {
            RoleName::Chat => &settings.roles.chat,
            RoleName::Decision => &settings.roles.decision,
            RoleName::Embedding => &settings.roles.embedding,
            RoleName::Rerank => &settings.roles.rerank,
        }
    }

    /// The model set up for `role`, or `None` when the role is not set up.
    pub fn target(&self, role: RoleName) -> Result<Option<Target>> {
        let settings = self.settings()?;
        let rm = Self::role_model(&settings, role);
        if !rm.is_set() {
            return Ok(None);
        }
        let store = self.store();
        let provider = providers::provider(&store, &self.secrets, &rm.provider_id)?;
        let profile = providers::profile(&store, &rm.provider_id, &rm.model)?;
        Ok(Some(Target {
            provider,
            model: rm.model.clone(),
            profile,
            thinking: providers::thinking(&rm.thinking),
        }))
    }

    pub fn require(&self, role: RoleName) -> Result<Target> {
        self.target(role)?
            .ok_or_else(|| Error::Setup(format!("请先在「设置 → 模型分配」中选择{}", role.label())))
    }
}
