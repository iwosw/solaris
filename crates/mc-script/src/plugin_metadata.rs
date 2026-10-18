//! Plugin-deployment metadata: what one deployed package declares about itself.
//!
//! These are the runtime-independent facts of one deployment - the ones core
//! needs to open a world against it, stage its client bundles and report what it
//! contributes - owned here so a consumer reads them without a script runtime
//! linked in. Nothing here knows how a rule was evaluated.
//!
//! The component host materializes this metadata from the package it validated
//! before startup. Client bundles and generic worldgen profiles share this
//! representation with the server's startup and Loader consumers.
//!
//! Fields are private and every read goes through an accessor. The constructors
//! take the parts a runtime already validated during discovery; they are the way
//! a runtime materializes that result, not a seal that stops anyone building a
//! descriptor without checking it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginWorldgenOreProfile {
    RealisticDeposits,
}

impl PluginWorldgenOreProfile {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RealisticDeposits => "realistic_deposits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientLoader {
    Fabric,
    NeoForge,
    Forge,
    Native,
}

impl ClientLoader {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::NeoForge => "neoforge",
            Self::Forge => "forge",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientContentKind {
    Blocks,
    Items,
    Views,
    ViewActions,
    Assets,
    WorldPreviews,
    WorldSelection,
    Sounds,
}

impl ClientContentKind {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Items => "items",
            Self::Views => "views",
            Self::ViewActions => "view_actions",
            Self::Assets => "assets",
            Self::WorldPreviews => "world_previews",
            Self::WorldSelection => "world_selection",
            Self::Sounds => "sounds",
        }
    }

    /// The permission a bundle needs for this content kind.
    ///
    /// Validation and the client loader both ask this question, so the answer
    /// stays a property of the content kind instead of being restated per caller.
    #[must_use]
    pub const fn required_permission(self) -> ClientPermission {
        match self {
            Self::Blocks => ClientPermission::RegisterBlocks,
            Self::Items => ClientPermission::RegisterItems,
            Self::Views => ClientPermission::PresentViews,
            Self::ViewActions => ClientPermission::SendViewActions,
            Self::Assets => ClientPermission::LoadAssets,
            Self::WorldPreviews => ClientPermission::PresentWorldPreviews,
            Self::WorldSelection => ClientPermission::SendWorldSelection,
            Self::Sounds => ClientPermission::PlaySounds,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ClientPermission {
    RegisterBlocks,
    RegisterItems,
    PresentViews,
    SendViewActions,
    LoadAssets,
    PresentWorldPreviews,
    SendWorldSelection,
    PlaySounds,
}

impl ClientPermission {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::RegisterBlocks => "register_blocks",
            Self::RegisterItems => "register_items",
            Self::PresentViews => "present_views",
            Self::SendViewActions => "send_view_actions",
            Self::LoadAssets => "load_assets",
            Self::PresentWorldPreviews => "present_world_previews",
            Self::SendWorldSelection => "send_world_selection",
            Self::PlaySounds => "play_sounds",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBundle {
    owner_plugin_id: String,
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: u64,
    artifact_path: PathBuf,
    artifact_bytes: Arc<[u8]>,
    loaders: Vec<ClientLoader>,
    content: Vec<ClientContentKind>,
    permissions: Vec<ClientPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginDeployment {
    ServerOnly,
    ServerAndClient,
}

impl PluginDeployment {
    #[must_use]
    pub const fn contract_name(self) -> &'static str {
        match self {
            Self::ServerOnly => "server_only",
            Self::ServerAndClient => "server_and_client",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientBundleDiscovery<'a> {
    id: &'a str,
    version: &'a str,
    artifact: &'a str,
    sha256: &'a str,
    size_bytes: u64,
    loaders: Vec<&'static str>,
    content: Vec<&'static str>,
    permissions: Vec<&'static str>,
}

impl<'a> ClientBundleDiscovery<'a> {
    /// Build one already-validated discovery row for one client bundle.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &'a str,
        version: &'a str,
        artifact: &'a str,
        sha256: &'a str,
        size_bytes: u64,
        loaders: Vec<&'static str>,
        content: Vec<&'static str>,
        permissions: Vec<&'static str>,
    ) -> Self {
        Self {
            id,
            version,
            artifact,
            sha256,
            size_bytes,
            loaders,
            content,
            permissions,
        }
    }
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    #[must_use]
    pub const fn version(&self) -> &'a str {
        self.version
    }

    #[must_use]
    pub const fn artifact(&self) -> &'a str {
        self.artifact
    }

    #[must_use]
    pub const fn sha256(&self) -> &'a str {
        self.sha256
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub fn loaders(&self) -> &[&'static str] {
        &self.loaders
    }

    #[must_use]
    pub fn content(&self) -> &[&'static str] {
        &self.content
    }

    #[must_use]
    pub fn permissions(&self) -> &[&'static str] {
        &self.permissions
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PluginDiscovery<'a> {
    id: &'a str,
    deployment: PluginDeployment,
    supported_loaders: Vec<&'static str>,
    permissions: Vec<&'static str>,
    total_artifact_bytes: u64,
    client_bundles: Vec<ClientBundleDiscovery<'a>>,
}

impl<'a> PluginDiscovery<'a> {
    /// Build one already-validated discovery row for one deployed plugin.
    #[must_use]
    pub fn new(
        id: &'a str,
        deployment: PluginDeployment,
        supported_loaders: Vec<&'static str>,
        permissions: Vec<&'static str>,
        total_artifact_bytes: u64,
        client_bundles: Vec<ClientBundleDiscovery<'a>>,
    ) -> Self {
        Self {
            id,
            deployment,
            supported_loaders,
            permissions,
            total_artifact_bytes,
            client_bundles,
        }
    }
    #[must_use]
    pub const fn id(&self) -> &'a str {
        self.id
    }

    #[must_use]
    pub const fn deployment(&self) -> PluginDeployment {
        self.deployment
    }

    #[must_use]
    pub fn supported_loaders(&self) -> &[&'static str] {
        &self.supported_loaders
    }

    #[must_use]
    pub fn permissions(&self) -> &[&'static str] {
        &self.permissions
    }

    #[must_use]
    pub const fn total_artifact_bytes(&self) -> u64 {
        self.total_artifact_bytes
    }

    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundleDiscovery<'a>] {
        &self.client_bundles
    }
}

impl ClientBundle {
    /// Build one already-validated bundle descriptor.
    ///
    /// Only discovery calls this: the artifact bytes and the sha256 are the
    /// validated ones, never a guest-supplied claim.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner_plugin_id: impl Into<String>,
        id: impl Into<String>,
        version: impl Into<String>,
        artifact: impl Into<String>,
        sha256: impl Into<String>,
        size_bytes: u64,
        artifact_path: PathBuf,
        artifact_bytes: Arc<[u8]>,
        loaders: Vec<ClientLoader>,
        content: Vec<ClientContentKind>,
        permissions: Vec<ClientPermission>,
    ) -> Self {
        Self {
            owner_plugin_id: owner_plugin_id.into(),
            id: id.into(),
            version: version.into(),
            artifact: artifact.into(),
            sha256: sha256.into(),
            size_bytes,
            artifact_path,
            artifact_bytes,
            loaders,
            content,
            permissions,
        }
    }
    #[must_use]
    pub fn owner_plugin_id(&self) -> &str {
        &self.owner_plugin_id
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    #[must_use]
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    pub fn artifact_path(&self) -> &Path {
        &self.artifact_path
    }

    #[must_use]
    pub fn artifact_bytes(&self) -> &[u8] {
        &self.artifact_bytes
    }

    #[must_use]
    pub fn artifact_bytes_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.artifact_bytes)
    }

    #[must_use]
    pub fn loaders(&self) -> &[ClientLoader] {
        &self.loaders
    }

    #[must_use]
    pub fn content(&self) -> &[ClientContentKind] {
        &self.content
    }

    #[must_use]
    pub fn permissions(&self) -> &[ClientPermission] {
        &self.permissions
    }

    #[must_use]
    pub fn cache_key(&self) -> String {
        format!(
            "{}:{}/{}/{}",
            self.owner_plugin_id, self.id, self.version, self.sha256
        )
    }
}
