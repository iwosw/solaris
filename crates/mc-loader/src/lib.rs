use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, OnceLock};

use bytes::BufMut;
use mc_data::Identifier;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LOADER_PROTOCOL_VERSION: u16 = 3;
pub const MAX_LOADER_MANIFEST_BYTES: usize = 32_767;
pub const MAX_LOADER_VIEW_MESSAGE_BYTES: usize = 64 * 1024;
pub const LOADER_ARTIFACT_CHUNK_BYTES: usize = 30 * 1024;
const LOADER_ARTIFACT_INDEX_PATH: &str = "solaris-client.json";
const MAX_LOADER_ARTIFACT_INDEX_BYTES: u64 = 64 * 1024;
const MAX_LOADER_BLOCK_NAME_BYTES: usize = 128;
const MAX_LOADER_BLOCKS: usize = 8;
const LOADER_VIEW_CHANNEL: &str = "solaris:loader/view";
const LOADER_VIEW_ACTION_CHANNEL: &str = "solaris:loader/view_action";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderPlatform {
    Fabric,
    #[serde(rename = "neoforge")]
    NeoForge,
    Forge,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderContentKind {
    Blocks,
    Items,
    Views,
    ViewActions,
    Assets,
    WorldPreviews,
    WorldSelection,
    Sounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoaderPermission {
    RegisterBlocks,
    RegisterItems,
    PresentViews,
    SendViewActions,
    LoadAssets,
    PresentWorldPreviews,
    SendWorldSelection,
    PlaySounds,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderBundle {
    pub owner: String,
    pub id: String,
    pub version: String,
    pub artifact: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub loaders: Vec<LoaderPlatform>,
    pub content: Vec<LoaderContentKind>,
    pub permissions: Vec<LoaderPermission>,
    pub cache_key: String,
    /// Server-local admission policy. It stays out of the wire manifest because
    /// clients only acknowledge bundles selected for their platform.
    #[serde(skip)]
    pub required: bool,
    /// Screen keys this bundle's verified artifact index declares. Server-local
    /// routing input; never crosses the wire and never replaces the artifact.
    #[serde(skip)]
    pub view_kinds: Vec<mc_script::ScriptClientViewKey>,
    /// Canonical server-local source; never crosses the wire.
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
    /// Immutable artifact bytes verified at startup; never crosses the wire as manifest data.
    #[serde(skip)]
    pub artifact_bytes: Option<Arc<[u8]>>,
    /// Carrier-backed custom block this bundle declares; never crosses the wire.
    #[serde(skip)]
    pub block: Option<CarrierBlockDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderManifest {
    pub protocol: u16,
    pub bundles: Vec<LoaderBundle>,
}

impl LoaderManifest {
    pub fn from_script_bundles(
        bundles: &[mc_script::ClientBundle],
    ) -> Result<Self, LoaderHandshakeError> {
        let bundles = bundles
            .iter()
            .map(|bundle| {
                let index = read_declared_index(bundle)?;
                let block = match index.as_ref().filter(|_| {
                    bundle
                        .content()
                        .contains(&mc_script::ClientContentKind::Blocks)
                }) {
                    Some(index) => Some(verify_declared_block(index, bundle.owner_plugin_id())?),
                    None => None,
                };
                let view_kinds = match index.as_ref().filter(|_| {
                    bundle
                        .content()
                        .contains(&mc_script::ClientContentKind::Views)
                }) {
                    Some(index) => declared_screen_keys(index, bundle.owner_plugin_id())?,
                    None => Vec::new(),
                };
                Ok(LoaderBundle {
                    owner: bundle.owner_plugin_id().to_owned(),
                    id: bundle.id().to_owned(),
                    version: bundle.version().to_owned(),
                    artifact: bundle.artifact().to_owned(),
                    sha256: bundle.sha256().to_owned(),
                    size_bytes: bundle.size_bytes(),
                    loaders: bundle
                        .loaders()
                        .iter()
                        .map(|loader| match loader {
                            mc_script::ClientLoader::Fabric => LoaderPlatform::Fabric,
                            mc_script::ClientLoader::NeoForge => LoaderPlatform::NeoForge,
                            mc_script::ClientLoader::Forge => LoaderPlatform::Forge,
                            mc_script::ClientLoader::Native => LoaderPlatform::Native,
                            _ => unreachable!("validated client loader"),
                        })
                        .collect(),
                    content: bundle
                        .content()
                        .iter()
                        .map(|content| match content {
                            mc_script::ClientContentKind::Blocks => LoaderContentKind::Blocks,
                            mc_script::ClientContentKind::Items => LoaderContentKind::Items,
                            mc_script::ClientContentKind::Views => LoaderContentKind::Views,
                            mc_script::ClientContentKind::ViewActions => {
                                LoaderContentKind::ViewActions
                            }
                            mc_script::ClientContentKind::Assets => LoaderContentKind::Assets,
                            mc_script::ClientContentKind::WorldPreviews => {
                                LoaderContentKind::WorldPreviews
                            }
                            mc_script::ClientContentKind::WorldSelection => {
                                LoaderContentKind::WorldSelection
                            }
                            mc_script::ClientContentKind::Sounds => LoaderContentKind::Sounds,
                            _ => unreachable!("validated client content kind"),
                        })
                        .collect(),
                    permissions: bundle
                        .permissions()
                        .iter()
                        .map(|permission| match permission {
                            mc_script::ClientPermission::RegisterBlocks => {
                                LoaderPermission::RegisterBlocks
                            }
                            mc_script::ClientPermission::RegisterItems => {
                                LoaderPermission::RegisterItems
                            }
                            mc_script::ClientPermission::PresentViews => {
                                LoaderPermission::PresentViews
                            }
                            mc_script::ClientPermission::SendViewActions => {
                                LoaderPermission::SendViewActions
                            }
                            mc_script::ClientPermission::LoadAssets => LoaderPermission::LoadAssets,
                            mc_script::ClientPermission::PresentWorldPreviews => {
                                LoaderPermission::PresentWorldPreviews
                            }
                            mc_script::ClientPermission::SendWorldSelection => {
                                LoaderPermission::SendWorldSelection
                            }
                            mc_script::ClientPermission::PlaySounds => LoaderPermission::PlaySounds,
                            _ => unreachable!("validated client permission"),
                        })
                        .collect(),
                    cache_key: bundle.cache_key(),
                    required: bundle.required(),
                    view_kinds,
                    source_path: Some(bundle.artifact_path().to_path_buf()),
                    artifact_bytes: Some(bundle.artifact_bytes_arc()),
                    block,
                })
            })
            .collect::<Result<Vec<_>, LoaderHandshakeError>>()?;
        let block_ids = bundles
            .iter()
            .filter_map(|bundle| bundle.block.as_ref().map(|block| block.id.as_str()))
            .collect::<BTreeSet<_>>();
        let block_count = bundles
            .iter()
            .filter(|bundle| bundle.block.is_some())
            .count();
        if block_count > MAX_LOADER_BLOCKS {
            return Err(LoaderHandshakeError::ArtifactIndex(format!(
                "Loader artifacts declare more than {MAX_LOADER_BLOCKS} blocks"
            )));
        }
        if block_ids.len() != block_count {
            return Err(LoaderHandshakeError::ArtifactIndex(
                "Loader artifacts declare duplicate block identities".to_owned(),
            ));
        }
        Ok(Self {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles,
        })
    }

    /// Require matching authorized artifacts for plugin-owned items only.
    /// Native ownership comes from the core catalog, never a namespace exemption.
    pub fn validate_custom_items<'a>(
        &self,
        definitions: impl IntoIterator<Item = &'a mc_data::item_components::CustomItemDefinition>,
        is_native_item: impl Fn(&Identifier) -> bool,
    ) -> Result<(), LoaderHandshakeError> {
        for definition in definitions {
            if is_native_item(&definition.id) {
                continue;
            }
            let mut found = false;
            for bundle in &self.bundles {
                if bundle.owner != definition.id.namespace()
                    || !bundle.content.contains(&LoaderContentKind::Items)
                    || !bundle
                        .permissions
                        .contains(&LoaderPermission::RegisterItems)
                {
                    continue;
                }
                let Some(bytes) = &bundle.artifact_bytes else {
                    continue;
                };
                let Some(path) = &bundle.source_path else {
                    continue;
                };
                let index =
                    read_index_from_artifact_bytes(bytes, path, bundle.size_bytes, &bundle.sha256)?;
                if index.items.iter().any(|item| {
                    item.id == definition.id.as_str()
                        && item.base_item == definition.carrier.as_str()
                        && item.name == definition.name
                }) {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(LoaderHandshakeError::ArtifactIndex(format!(
                    "custom item {} needs a matching Loader item and register_items permission from its owner",
                    definition.id
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, LoaderHandshakeError> {
        let payload = serde_json::to_vec(self)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
        if payload.len() > MAX_LOADER_MANIFEST_BYTES {
            return Err(LoaderHandshakeError::ManifestTooLarge {
                len: payload.len(),
                max: MAX_LOADER_MANIFEST_BYTES,
            });
        }
        Ok(payload)
    }

    pub fn validate_ack(&self, ack: &LoaderClientAck) -> Result<(), LoaderHandshakeError> {
        if ack.protocol != self.protocol {
            return Err(LoaderHandshakeError::Protocol {
                expected: self.protocol,
                actual: ack.protocol,
            });
        }
        if ack.loader_version.is_empty() || ack.loader_version.len() > 64 {
            return Err(LoaderHandshakeError::InvalidLoaderVersion);
        }
        let accepted = ack
            .accepted_permissions
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let cached = ack
            .cached_bundles
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for bundle in &self.bundles {
            if !bundle.loaders.contains(&ack.platform) {
                if bundle.required {
                    return Err(LoaderHandshakeError::UnsupportedPlatform {
                        owner: bundle.owner.clone(),
                        bundle: bundle.id.clone(),
                        platform: ack.platform,
                    });
                }
                continue;
            }
            if !cached.contains(bundle.cache_key.as_str()) {
                if bundle.required {
                    return Err(LoaderHandshakeError::BundleUnavailable {
                        owner: bundle.owner.clone(),
                        bundle: bundle.id.clone(),
                    });
                }
                continue;
            }
            if let Some(permission) = bundle
                .permissions
                .iter()
                .find(|permission| !accepted.contains(permission))
            {
                return Err(LoaderHandshakeError::PermissionDenied {
                    owner: bundle.owner.clone(),
                    bundle: bundle.id.clone(),
                    permission: *permission,
                });
            }
        }
        Ok(())
    }

    /// Bind a validated ack. Carrier states are server-dictated (ADR 0018), so
    /// clients negotiate none.
    pub fn bind_ack(&self, ack: &LoaderClientAck) -> Result<LoaderSession, LoaderHandshakeError> {
        self.validate_ack(ack)?;
        if !ack.carrier_block_state_ids.is_empty() {
            return Err(LoaderHandshakeError::UnexpectedBlockCarrierState);
        }
        Ok(LoaderSession {
            platform: ack.platform,
            loader_version: ack.loader_version.clone(),
        })
    }

    pub fn requested_artifact(
        &self,
        request: &LoaderArtifactRequest,
    ) -> Result<(&LoaderBundle, &[u8]), LoaderHandshakeError> {
        if request.protocol != self.protocol {
            return Err(LoaderHandshakeError::Protocol {
                expected: self.protocol,
                actual: request.protocol,
            });
        }
        let bundle = self
            .bundles
            .iter()
            .find(|bundle| bundle.cache_key == request.cache_key)
            .ok_or_else(|| LoaderHandshakeError::UnknownBundleRequest {
                cache_key: request.cache_key.clone(),
            })?;
        let bytes = bundle.artifact_bytes.as_deref().ok_or_else(|| {
            LoaderHandshakeError::ArtifactUnavailable {
                cache_key: request.cache_key.clone(),
            }
        })?;
        Ok((bundle, bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderClientAck {
    pub protocol: u16,
    pub platform: LoaderPlatform,
    pub loader_version: String,
    pub accepted_permissions: Vec<LoaderPermission>,
    pub cached_bundles: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub carrier_block_state_ids: BTreeMap<String, u32>,
}

impl LoaderClientAck {
    pub fn decode(payload: &[u8]) -> Result<Self, LoaderHandshakeError> {
        serde_json::from_slice(payload)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoaderSession {
    platform: LoaderPlatform,
    loader_version: String,
}

/// One carrier-backed custom block a verified bundle index declares (ADR 0018).
///
/// `carrier` is a vanilla block id naming Solaris' reserved carrier states and
/// `drop_item` the owner's custom item; core validates both against its
/// registries. Clients read the same fields from the artifact index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierBlockDeclaration {
    pub id: String,
    pub name: String,
    pub model: String,
    pub carrier: String,
    pub drop_item: String,
}

/// The carrier-backed block declared by one script bundle's verified artifact.
pub fn declared_carrier_block(
    bundle: &mc_script::ClientBundle,
) -> Result<Option<CarrierBlockDeclaration>, LoaderHandshakeError> {
    if !bundle
        .content()
        .contains(&mc_script::ClientContentKind::Blocks)
    {
        return Ok(None);
    }
    let index = read_declared_index(bundle)?.ok_or_else(|| {
        LoaderHandshakeError::ArtifactIndex("block bundle has no index".to_owned())
    })?;
    verify_declared_block(&index, bundle.owner_plugin_id()).map(Some)
}

impl LoaderSession {
    #[must_use]
    pub const fn platform(&self) -> LoaderPlatform {
        self.platform
    }

    #[must_use]
    pub fn loader_version(&self) -> &str {
        &self.loader_version
    }
}

/// The part of one schema-2 artifact index the server routes on. The Loader
/// validates and activates the rest (previews, items, assets, sounds) from the
/// same artifact, so unknown fields are tolerated here rather than re-owned.
#[derive(Deserialize)]
struct LoaderArtifactIndex {
    schema: u16,
    #[serde(default)]
    screens: Vec<LoaderArtifactScreen>,
    #[serde(default)]
    blocks: Vec<LoaderArtifactBlock>,
    #[serde(default)]
    items: Vec<LoaderArtifactItem>,
}

/// The routing identity of one declared screen. The Loader owns the rest of the
/// screen contract (title, widgets); the server only needs to know which plugin
/// answers a key-driven request for this plugin-owned key.
#[derive(Deserialize)]
struct LoaderArtifactScreen {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoaderArtifactBlock {
    id: String,
    model: String,
    name: String,
    carrier: String,
    drop_item: String,
}

#[derive(Deserialize)]
struct LoaderArtifactItem {
    id: String,
    base_item: String,
    name: String,
}

fn read_declared_index(
    bundle: &mc_script::ClientBundle,
) -> Result<Option<LoaderArtifactIndex>, LoaderHandshakeError> {
    let declared = |content| bundle.content().contains(&content);
    if !declared(mc_script::ClientContentKind::Blocks)
        && !declared(mc_script::ClientContentKind::Items)
        && !declared(mc_script::ClientContentKind::Views)
    {
        return Ok(None);
    }
    read_index_from_artifact_bytes(
        bundle.artifact_bytes(),
        bundle.artifact_path(),
        bundle.size_bytes(),
        bundle.sha256(),
    )
    .map(Some)
}

/// The one block identity a block bundle declares, verified against its owner.
fn verify_declared_block(
    index: &LoaderArtifactIndex,
    owner: &str,
) -> Result<CarrierBlockDeclaration, LoaderHandshakeError> {
    let [block] = index.blocks.as_slice() else {
        return Err(LoaderHandshakeError::ArtifactIndex(
            "block bundle must declare exactly one block identity".to_owned(),
        ));
    };
    require_owned_block_id(&block.id, owner)?;
    require_owned_block_id(&block.drop_item, owner)?;
    if block.model.is_empty()
        || block.name.is_empty()
        || block.name.len() > MAX_LOADER_BLOCK_NAME_BYTES
        || Identifier::parse(block.carrier.clone()).is_err()
    {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "Loader block model and carrier must be valid and name must contain 1..={MAX_LOADER_BLOCK_NAME_BYTES} bytes"
        )));
    }
    Ok(CarrierBlockDeclaration {
        id: block.id.clone(),
        name: block.name.clone(),
        model: block.model.clone(),
        carrier: block.carrier.clone(),
        drop_item: block.drop_item.clone(),
    })
}

/// The plugin-owned screen keys a verified view bundle declares for routing.
fn declared_screen_keys(
    index: &LoaderArtifactIndex,
    owner: &str,
) -> Result<Vec<mc_script::ScriptClientViewKey>, LoaderHandshakeError> {
    if index.screens.is_empty() {
        return Err(LoaderHandshakeError::ArtifactIndex(
            "view bundle must declare at least one screen".to_owned(),
        ));
    }
    let mut keys: Vec<mc_script::ScriptClientViewKey> = Vec::with_capacity(index.screens.len());
    for screen in &index.screens {
        if keys.iter().any(|key| key.as_str() == screen.id) {
            continue;
        }
        let key = mc_script::ScriptClientViewKey::try_new(&screen.id)
            .map_err(|error| LoaderHandshakeError::ArtifactIndex(error.to_string()))?;
        if key.as_str().split_once(':').map(|(namespace, _)| namespace) != Some(owner) {
            return Err(LoaderHandshakeError::ArtifactIndex(format!(
                "Loader screen key {:?} must be owned by {owner}",
                screen.id
            )));
        }
        keys.push(key);
    }
    Ok(keys)
}

fn read_index_from_artifact_bytes(
    bytes: &[u8],
    artifact_path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<LoaderArtifactIndex, LoaderHandshakeError> {
    if bytes.len() as u64 != expected_size
        || format!("{:x}", Sha256::digest(bytes)) != expected_sha256
    {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} changed after plugin artifact verification",
            artifact_path.display()
        )));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        LoaderHandshakeError::ArtifactIndex(format!(
            "opening {} as ZIP: {error}",
            artifact_path.display()
        ))
    })?;
    let mut entry = archive.by_index(0).map_err(|error| {
        LoaderHandshakeError::ArtifactIndex(format!(
            "reading the first entry from {}: {error}",
            artifact_path.display()
        ))
    })?;
    if entry.name() != LOADER_ARTIFACT_INDEX_PATH || entry.is_dir() {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} must begin with {LOADER_ARTIFACT_INDEX_PATH}",
            artifact_path.display()
        )));
    }
    if entry.size() > MAX_LOADER_ARTIFACT_INDEX_BYTES {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} exceeds {MAX_LOADER_ARTIFACT_INDEX_BYTES} bytes",
            LOADER_ARTIFACT_INDEX_PATH
        )));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .by_ref()
        .take(MAX_LOADER_ARTIFACT_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| LoaderHandshakeError::ArtifactIndex(error.to_string()))?;
    if bytes.len() as u64 > MAX_LOADER_ARTIFACT_INDEX_BYTES {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "{} exceeds {MAX_LOADER_ARTIFACT_INDEX_BYTES} bytes",
            LOADER_ARTIFACT_INDEX_PATH
        )));
    }
    let index: LoaderArtifactIndex = serde_json::from_slice(&bytes)
        .map_err(|error| LoaderHandshakeError::ArtifactIndex(error.to_string()))?;
    if index.schema != 2 {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "unsupported Loader artifact index schema {}",
            index.schema
        )));
    }
    Ok(index)
}

fn require_owned_block_id(id: &str, owner: &str) -> Result<(), LoaderHandshakeError> {
    let prefix = format!("{owner}:");
    if id.len() > 128 || !id.starts_with(&prefix) || id.len() == prefix.len() {
        return Err(LoaderHandshakeError::ArtifactIndex(format!(
            "Loader block identity {id:?} must be owned by {owner}"
        )));
    }
    for (index, byte) in id.bytes().enumerate() {
        let separator = index == owner.len() && byte == b':';
        let allowed = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'_' | b'.' | b'-')
            || (index > owner.len() && byte == b'/');
        if !separator && !allowed {
            return Err(LoaderHandshakeError::ArtifactIndex(format!(
                "Loader block identity {id:?} contains invalid characters"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderArtifactRequest {
    pub protocol: u16,
    pub cache_key: String,
}

/// One wire-3 view action admitted from a Loader client.
#[derive(Debug, Clone, PartialEq)]
pub struct LoaderViewActionRequest {
    pub view_instance_id: String,
    pub view_revision: u64,
    pub action_id: String,
    pub action_sequence: u64,
    pub fields: Vec<mc_script::ScriptClientViewField>,
    pub selection_token: Option<String>,
}

/// One decoded wire-3 client request from the Loader action channel.
#[derive(Debug, Clone, PartialEq)]
pub enum LoaderViewClientRequest {
    /// Explicit key-driven open request: `view_request { request_kind }`.
    ViewRequest {
        request_kind: mc_script::ScriptClientViewKey,
    },
    Action(LoaderViewActionRequest),
    CancelSelection {
        selection_context_id: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewField {
    id: String,
    #[serde(default)]
    number: Option<f64>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    selected: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewAction {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    view_instance_id: String,
    view_revision: u64,
    action_id: String,
    action_sequence: u64,
    #[serde(default)]
    fields: Vec<WireViewField>,
    #[serde(default)]
    selection_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireViewRequest {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    request_kind: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCancelSelection {
    protocol: u16,
    #[allow(dead_code)]
    message: String,
    selection_context_id: String,
}

fn decode_wire_view_field(
    field: WireViewField,
) -> Result<mc_script::ScriptClientViewField, LoaderHandshakeError> {
    let typed =
        field.number.is_some() as u8 + field.text.is_some() as u8 + field.selected.is_some() as u8;
    if typed != 1 {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view field {:?} must carry exactly one typed value",
            field.id
        )));
    }
    let parsed = if let Some(number) = field.number {
        mc_script::ScriptClientViewField::try_number(&field.id, number)
    } else if let Some(text) = field.text {
        mc_script::ScriptClientViewField::try_text(&field.id, text)
    } else {
        mc_script::ScriptClientViewField::try_selected(
            &field.id,
            field.selected.as_deref().unwrap_or_default(),
        )
    };
    parsed.map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
}

/// Decode one closed-schema wire-3 client request; unknown fields, unknown
/// messages and a wrong protocol all fail closed.
pub fn decode_loader_view_client_request(
    payload: &[u8],
) -> Result<LoaderViewClientRequest, LoaderHandshakeError> {
    if payload.is_empty() || payload.len() > MAX_LOADER_VIEW_MESSAGE_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view request size is outside 1..={MAX_LOADER_VIEW_MESSAGE_BYTES}"
        )));
    }
    let document: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
    let message = document
        .get("message")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| LoaderHandshakeError::Malformed("view request has no message".to_owned()))?;
    match message {
        "view_action" => {
            let action: WireViewAction = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(action.protocol)?;
            if action.fields.len() > mc_script::MAX_CLIENT_VIEW_FIELDS {
                return Err(LoaderHandshakeError::Malformed(
                    "view action fields exceed the limit".to_owned(),
                ));
            }
            let fields = action
                .fields
                .into_iter()
                .map(decode_wire_view_field)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LoaderViewClientRequest::Action(LoaderViewActionRequest {
                view_instance_id: action.view_instance_id,
                view_revision: action.view_revision,
                action_id: action.action_id,
                action_sequence: action.action_sequence,
                fields,
                selection_token: action.selection_token,
            }))
        }
        "cancel_selection" => {
            let cancel: WireCancelSelection = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(cancel.protocol)?;
            Ok(LoaderViewClientRequest::CancelSelection {
                selection_context_id: cancel.selection_context_id,
            })
        }
        "view_request" => {
            let request: WireViewRequest = serde_json::from_value(document)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            require_view_protocol(request.protocol)?;
            let request_kind = mc_script::ScriptClientViewKey::try_new(&request.request_kind)
                .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
            Ok(LoaderViewClientRequest::ViewRequest { request_kind })
        }
        other => Err(LoaderHandshakeError::Malformed(format!(
            "unknown Loader view message {other:?}"
        ))),
    }
}

fn require_view_protocol(protocol: u16) -> Result<(), LoaderHandshakeError> {
    if protocol != LOADER_PROTOCOL_VERSION {
        return Err(LoaderHandshakeError::Protocol {
            expected: LOADER_PROTOCOL_VERSION,
            actual: protocol,
        });
    }
    Ok(())
}

/// One wire-3 server-to-client view message.
#[derive(Debug, Clone, Copy)]
pub enum LoaderViewWireMessage<'a> {
    Open {
        view_instance_id: &'a str,
        revision: u64,
        view_id: &'a str,
        title: &'a str,
        model: &'a mc_script::ScriptClientViewModel,
    },
    Present {
        view_instance_id: &'a str,
        revision: u64,
        model: &'a mc_script::ScriptClientViewModel,
    },
    Close {
        view_instance_id: &'a str,
    },
}

/// Encode one wire-3 message, failing closed when it exceeds the packet bound.
pub fn encode_loader_view_message(
    message: &LoaderViewWireMessage<'_>,
) -> Result<Vec<u8>, LoaderHandshakeError> {
    let document = match message {
        LoaderViewWireMessage::Open {
            view_instance_id,
            revision,
            view_id,
            title,
            model,
        } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "open_view",
            "view_instance_id": view_instance_id,
            "revision": revision,
            "view_id": view_id,
            "title": title,
            "model": model,
        }),
        LoaderViewWireMessage::Present {
            view_instance_id,
            revision,
            model,
        } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "present_view",
            "view_instance_id": view_instance_id,
            "revision": revision,
            "model": model,
        }),
        LoaderViewWireMessage::Close { view_instance_id } => serde_json::json!({
            "protocol": LOADER_PROTOCOL_VERSION,
            "message": "close_view",
            "view_instance_id": view_instance_id,
        }),
    };
    let payload = serde_json::to_vec(&document)
        .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))?;
    if payload.len() > MAX_LOADER_VIEW_MESSAGE_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "view message is {} bytes, maximum is {MAX_LOADER_VIEW_MESSAGE_BYTES}",
            payload.len()
        )));
    }
    Ok(payload)
}

impl LoaderArtifactRequest {
    pub fn decode(payload: &[u8]) -> Result<Self, LoaderHandshakeError> {
        serde_json::from_slice(payload)
            .map_err(|error| LoaderHandshakeError::Malformed(error.to_string()))
    }
}

pub fn encode_artifact_chunk(
    cache_key: &str,
    offset: u64,
    last: bool,
    bytes: &[u8],
) -> Result<Vec<u8>, LoaderHandshakeError> {
    let cache_key = cache_key.as_bytes();
    if cache_key.is_empty() || cache_key.len() > u16::MAX as usize {
        return Err(LoaderHandshakeError::Malformed(
            "artifact cache key length is outside 1..=65535".to_owned(),
        ));
    }
    if bytes.is_empty() || bytes.len() > LOADER_ARTIFACT_CHUNK_BYTES {
        return Err(LoaderHandshakeError::Malformed(format!(
            "artifact chunk length is outside 1..={LOADER_ARTIFACT_CHUNK_BYTES}"
        )));
    }
    let mut payload = Vec::with_capacity(2 + 2 + cache_key.len() + 8 + 1 + bytes.len());
    payload.put_u16(LOADER_PROTOCOL_VERSION);
    payload
        .put_u16(u16::try_from(cache_key.len()).expect("cache key length was checked against u16"));
    payload.put_slice(cache_key);
    payload.put_u64(offset);
    payload.put_u8(u8::from(last));
    payload.put_slice(bytes);
    Ok(payload)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoaderHandshakeError {
    #[error("loader payload is malformed: {0}")]
    Malformed(String),
    #[error("loader manifest is {len} bytes, maximum is {max}")]
    ManifestTooLarge { len: usize, max: usize },
    #[error("loader protocol mismatch: expected {expected}, got {actual}")]
    Protocol { expected: u16, actual: u16 },
    #[error("loader version must contain 1..=64 bytes")]
    InvalidLoaderVersion,
    #[error("bundle {owner}:{bundle} does not support platform {platform:?}")]
    UnsupportedPlatform {
        owner: String,
        bundle: String,
        platform: LoaderPlatform,
    },
    #[error("bundle {owner}:{bundle} permission {permission:?} was not accepted")]
    PermissionDenied {
        owner: String,
        bundle: String,
        permission: LoaderPermission,
    },
    #[error("bundle {owner}:{bundle} is not present in the client cache")]
    BundleUnavailable { owner: String, bundle: String },
    #[error("client requested unknown loader cache identity {cache_key}")]
    UnknownBundleRequest { cache_key: String },
    #[error("server artifact is unavailable for loader cache identity {cache_key}")]
    ArtifactUnavailable { cache_key: String },
    #[error("loader artifact index is invalid: {0}")]
    ArtifactIndex(String),
    #[error("client reported a Loader block carrier state; carriers are server-dictated")]
    UnexpectedBlockCarrierState,
}

#[must_use]
pub fn loader_manifest_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/manifest").expect("static loader manifest channel")
    })
}

pub fn loader_view_channel() -> &'static Identifier {
    static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::parse(LOADER_VIEW_CHANNEL).expect("static Solaris Loader view channel is valid")
    });
    &CHANNEL
}

pub fn loader_view_action_channel() -> &'static Identifier {
    static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::parse(LOADER_VIEW_ACTION_CHANNEL)
            .expect("static Solaris Loader view action channel is valid")
    });
    &CHANNEL
}

impl LoaderManifest {
    /// Every screen key a verified bundle lets this owner answer, with its
    /// owner. This is the server's routing table for key-driven client view
    /// requests; the artifact index stays the single authority it is read from.
    pub fn declared_view_kinds(
        &self,
    ) -> impl Iterator<Item = (&str, &mc_script::ScriptClientViewKey)> + '_ {
        self.bundles.iter().flat_map(|bundle| {
            bundle
                .view_kinds
                .iter()
                .map(move |kind| (bundle.owner.as_str(), kind))
        })
    }

    /// True when `plugin_id` owns a bundle declaring both the content kind and
    /// its permission pair. Permission revocation re-runs this check per action.
    pub fn plugin_grants(
        &self,
        plugin_id: &str,
        content: LoaderContentKind,
        permission: LoaderPermission,
    ) -> bool {
        self.bundles.iter().any(|bundle| {
            bundle.owner == plugin_id
                && bundle.content.contains(&content)
                && bundle.permissions.contains(&permission)
        })
    }
}

/// Server-to-client configuration probe, sent before known-pack negotiation
/// when every bundle is optional. Its payload is empty.
#[must_use]
pub fn loader_probe_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/probe").expect("static loader probe channel")
    })
}

/// Client-to-server answer to the probe: this client speaks the Loader. A Loader
/// client sends it (empty payload) while handling the probe, so it precedes the
/// client's known-pack reply. Vanilla clients never send it.
#[must_use]
pub fn loader_hello_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/hello").expect("static loader hello channel")
    })
}

#[must_use]
pub fn loader_ack_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL
        .get_or_init(|| Identifier::parse("solaris:loader/ack").expect("static loader ack channel"))
}

#[must_use]
pub fn loader_request_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/request").expect("static loader request channel")
    })
}

#[must_use]
pub fn loader_artifact_channel() -> &'static Identifier {
    static CHANNEL: OnceLock<Identifier> = OnceLock::new();
    CHANNEL.get_or_init(|| {
        Identifier::parse("solaris:loader/artifact").expect("static loader artifact channel")
    })
}

/// Encode the protocol-3 Loader sound payload shared by both networking hosts.
pub fn sound_payload(sound: &mc_script::ScriptClientSound) -> Vec<u8> {
    let playback = sound.playback();
    let mode = playback.map_or(0, |play| if play.position().is_some() { 2 } else { 1 });
    let mut payload = Vec::with_capacity(37 + sound.sound_id().len());
    payload.put_u16(LOADER_PROTOCOL_VERSION);
    payload.put_u8(mode);
    payload.put_u16(sound.sound_id().len() as u16);
    payload.extend_from_slice(sound.sound_id().as_bytes());
    if let Some(play) = playback {
        payload.put_f32(play.volume());
        payload.put_f32(play.pitch());
        if let Some(position) = play.position() {
            payload.put_f64(position.x());
            payload.put_f64(position.y());
            payload.put_f64(position.z());
        }
    }
    payload
}

#[must_use]
pub fn loader_sound_channel() -> &'static Identifier {
    static CHANNEL: LazyLock<Identifier> = LazyLock::new(|| {
        Identifier::parse("solaris:loader/sound").expect("static Loader sound channel")
    });
    &CHANNEL
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    fn manifest() -> LoaderManifest {
        LoaderManifest {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles: vec![LoaderBundle {
                owner: "example".to_owned(),
                id: "rich-content".to_owned(),
                version: "1".to_owned(),
                artifact: "client/rich.zip".to_owned(),
                sha256: "a".repeat(64),
                size_bytes: 128,
                loaders: vec![
                    LoaderPlatform::Fabric,
                    LoaderPlatform::NeoForge,
                    LoaderPlatform::Forge,
                    LoaderPlatform::Native,
                ],
                content: vec![
                    LoaderContentKind::Blocks,
                    LoaderContentKind::Items,
                    LoaderContentKind::Views,
                    LoaderContentKind::Assets,
                    LoaderContentKind::ViewActions,
                ],
                permissions: vec![
                    LoaderPermission::RegisterBlocks,
                    LoaderPermission::RegisterItems,
                    LoaderPermission::PresentViews,
                    LoaderPermission::LoadAssets,
                    LoaderPermission::SendViewActions,
                ],
                cache_key: format!("example:rich-content/1/{}", "a".repeat(64)),
                required: false,
                view_kinds: Vec::new(),
                source_path: None,
                artifact_bytes: None,
                block: None,
            }],
        }
    }

    #[test]
    fn manifest_and_ack_round_trip_all_loader_platforms() {
        let manifest = manifest();
        let encoded = manifest.encode().unwrap();
        assert_eq!(
            serde_json::from_slice::<LoaderManifest>(&encoded).unwrap(),
            manifest
        );
        for platform in [
            LoaderPlatform::Fabric,
            LoaderPlatform::NeoForge,
            LoaderPlatform::Forge,
            LoaderPlatform::Native,
        ] {
            let ack = LoaderClientAck {
                protocol: LOADER_PROTOCOL_VERSION,
                platform,
                loader_version: "prototype".to_owned(),
                accepted_permissions: manifest.bundles[0].permissions.clone(),
                cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
                carrier_block_state_ids: BTreeMap::new(),
            };
            manifest.validate_ack(&ack).unwrap();
        }
    }

    #[test]
    fn optional_java_bundle_allows_native_ack_without_a_bound_bundle() {
        let mut manifest = manifest();
        manifest.bundles[0]
            .loaders
            .retain(|platform| *platform != LoaderPlatform::Native);

        let native_ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Native,
            loader_version: "solaris-client-0.0.8".to_owned(),
            accepted_permissions: Vec::new(),
            cached_bundles: Vec::new(),
            carrier_block_state_ids: BTreeMap::new(),
        };
        let session = manifest.bind_ack(&native_ack).unwrap();
        assert_eq!(session.platform(), LoaderPlatform::Native);
    }

    #[test]
    fn java_ack_binds_optional_java_bundle() {
        let mut manifest = manifest();
        manifest.bundles[0]
            .loaders
            .retain(|platform| *platform != LoaderPlatform::Native);

        let java_ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "fabric-26.1.2".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        };
        let session = manifest.bind_ack(&java_ack).unwrap();
        assert_eq!(session.platform(), LoaderPlatform::Fabric);
    }

    #[test]
    fn required_java_bundle_refuses_native_ack() {
        let mut manifest = manifest();
        manifest.bundles[0]
            .loaders
            .retain(|platform| *platform != LoaderPlatform::Native);
        manifest.bundles[0].required = true;

        let native_ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Native,
            loader_version: "solaris-client-0.0.8".to_owned(),
            accepted_permissions: Vec::new(),
            cached_bundles: Vec::new(),
            carrier_block_state_ids: BTreeMap::new(),
        };
        assert!(matches!(
            manifest.bind_ack(&native_ack),
            Err(LoaderHandshakeError::UnsupportedPlatform {
                platform: LoaderPlatform::Native,
                ..
            })
        ));
    }

    #[test]
    fn ack_requires_permission_and_exact_cache_identity() {
        let mut manifest = manifest();
        let mut ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::NeoForge,
            loader_version: "26.1.2.76".to_owned(),
            accepted_permissions: Vec::new(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        };
        assert!(matches!(
            manifest.validate_ack(&ack),
            Err(LoaderHandshakeError::PermissionDenied { .. })
        ));
        // The fixture bundle is optional: granting permissions binds it, and
        // clearing the cache then unbinds it instead of failing the ack.
        ack.accepted_permissions = manifest.bundles[0].permissions.clone();
        manifest.validate_ack(&ack).unwrap();
        ack.cached_bundles.clear();
        manifest.validate_ack(&ack).unwrap();
        // Required bundles restore the old strict contract: the selected
        // platform must be cached before permissions are even checked.
        manifest.bundles[0].required = true;
        assert!(matches!(
            manifest.validate_ack(&ack),
            Err(LoaderHandshakeError::BundleUnavailable { .. })
        ));
    }

    #[test]
    fn client_may_not_negotiate_block_carrier_states() {
        let manifest = manifest();
        let mut ack = LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "26.1.2".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        };
        let session = manifest.bind_ack(&ack).unwrap();
        assert_eq!(session.platform(), LoaderPlatform::Fabric);
        assert_eq!(session.loader_version(), "26.1.2");
        ack.carrier_block_state_ids
            .insert("example:ruby_block".to_owned(), 321);
        assert_eq!(
            manifest.bind_ack(&ack),
            Err(LoaderHandshakeError::UnexpectedBlockCarrierState)
        );
    }

    #[test]
    fn artifact_index_supplies_one_exact_owner_block_identity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("block.bundle");
        let file = File::create(&path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        archive
            .start_file(
                LOADER_ARTIFACT_INDEX_PATH,
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                br#"{"schema":2,"screens":[{"id":"example:showcase","title":"Showcase","widgets":[]}],"blocks":[{"id":"example:ruby_block","model":"example:block/ruby_block","name":"Ruby Block","carrier":"minecraft:black_glazed_terracotta","drop_item":"example:ruby_block"}],"items":[],"assets":[]}"#,
            )
            .unwrap();
        archive.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let size = bytes.len() as u64;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));

        let index = read_index_from_artifact_bytes(&bytes, &path, size, &sha256).unwrap();
        assert_eq!(
            verify_declared_block(&index, "example").unwrap().id,
            "example:ruby_block"
        );
        assert!(matches!(
            verify_declared_block(&index, "other"),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
        assert!(matches!(
            read_index_from_artifact_bytes(&bytes, &path, size, &"0".repeat(64)),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
    }

    #[test]
    fn native_items_allow_empty_loader_but_plugin_items_still_require_artifacts() {
        let manifest = LoaderManifest {
            protocol: LOADER_PROTOCOL_VERSION,
            bundles: Vec::new(),
        };
        let native = [
            "arming_sword",
            "longsword",
            "spear",
            "war_bow",
            "mail_hauberk",
            "pavise",
        ]
        .map(|name| mc_data::item_components::CustomItemDefinition {
            id: Identifier::parse(format!("solaris-settlements:{name}")).unwrap(),
            carrier: Identifier::parse("minecraft:paper").unwrap(),
            name: name.to_owned(),
            crafting_ingredient: None,
            facts: Default::default(),
        });
        let is_native = |id: &Identifier| native.iter().any(|item| item.id == *id);
        manifest.validate_custom_items(&native, is_native).unwrap();
        let mut plugin = native[0].clone();
        for id in ["example:ruby", "solaris-settlements:plugin_only"] {
            plugin.id = Identifier::parse(id).unwrap();
            assert!(matches!(
                manifest.validate_custom_items(std::iter::once(&plugin), is_native),
                Err(LoaderHandshakeError::ArtifactIndex(_))
            ));
        }
    }

    #[test]
    fn canonical_item_requires_matching_verified_artifact_and_registration_permission() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("items.bundle");
        let mut archive = zip::ZipWriter::new(File::create(&path).unwrap());
        archive
            .start_file(
                LOADER_ARTIFACT_INDEX_PATH,
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                br#"{"schema":2,"items":[{"id":"example:ruby","base_item":"minecraft:paper","name":"Ruby"}]}"#,
            )
            .unwrap();
        archive.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut manifest = manifest();
        manifest.bundles[0].source_path = Some(path);
        manifest.bundles[0].size_bytes = bytes.len() as u64;
        manifest.bundles[0].sha256 = format!("{:x}", Sha256::digest(&bytes));
        manifest.bundles[0].artifact_bytes = Some(Arc::from(bytes));
        let definition = mc_data::item_components::CustomItemDefinition {
            id: Identifier::parse("example:ruby").unwrap(),
            carrier: Identifier::parse("minecraft:paper").unwrap(),
            name: "Ruby".to_owned(),
            crafting_ingredient: None,
            facts: mc_data::item_components::ItemFacts {
                max_stack_size: Some(16),
                ..Default::default()
            },
        };
        manifest
            .validate_custom_items(std::slice::from_ref(&definition), |_| false)
            .unwrap();
        manifest.bundles[0]
            .permissions
            .retain(|permission| *permission != LoaderPermission::RegisterItems);
        assert!(matches!(
            manifest.validate_custom_items(std::slice::from_ref(&definition), |_| false),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
        manifest.bundles[0]
            .permissions
            .push(LoaderPermission::RegisterItems);
        let mut wrong = definition.clone();
        wrong.name = "Another item".to_owned();
        assert!(matches!(
            manifest.validate_custom_items(std::slice::from_ref(&wrong), |_| false),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
    }

    #[test]
    fn exact_request_resolves_only_the_manifest_artifact() {
        let mut manifest = manifest();
        manifest.bundles[0].source_path = Some(PathBuf::from("/plugin/client/rich.zip"));
        manifest.bundles[0].artifact_bytes = Some(Arc::from(&b"verified artifact"[..]));
        let request = LoaderArtifactRequest {
            protocol: LOADER_PROTOCOL_VERSION,
            cache_key: manifest.bundles[0].cache_key.clone(),
        };

        let (bundle, bytes) = manifest.requested_artifact(&request).unwrap();

        assert_eq!(bundle.id, "rich-content");
        assert_eq!(bytes, b"verified artifact");
        let unknown = LoaderArtifactRequest {
            cache_key: format!("{}-other", request.cache_key),
            ..request
        };
        assert!(matches!(
            manifest.requested_artifact(&unknown),
            Err(LoaderHandshakeError::UnknownBundleRequest { .. })
        ));
    }

    #[test]
    fn artifact_chunk_encoding_is_bounded_and_big_endian() {
        let payload = encode_artifact_chunk("cache-key", 42, true, b"abc").unwrap();

        assert_eq!(&payload[0..2], &LOADER_PROTOCOL_VERSION.to_be_bytes());
        assert_eq!(&payload[2..4], &9_u16.to_be_bytes());
        assert_eq!(&payload[4..13], b"cache-key");
        assert_eq!(&payload[13..21], &42_u64.to_be_bytes());
        assert_eq!(payload[21], 1);
        assert_eq!(&payload[22..], b"abc");
        assert!(
            encode_artifact_chunk(
                "cache-key",
                0,
                false,
                &vec![0; LOADER_ARTIFACT_CHUNK_BYTES + 1],
            )
            .is_err()
        );
    }

    #[test]
    fn view_request_decoding_is_closed_and_protocol_fenced() {
        let action = br#"{"protocol":3,"message":"view_action","view_instance_id":"solaris:view-1","view_revision":2,"action_id":"example:place","action_sequence":1,"fields":[{"id":"count","number":4}],"selection_token":"example:ctx-1"}"#;
        let decoded = decode_loader_view_client_request(action).unwrap();
        let LoaderViewClientRequest::Action(action) = decoded else {
            panic!("expected a view action");
        };
        assert_eq!(action.view_instance_id, "solaris:view-1");
        assert_eq!(action.view_revision, 2);
        assert_eq!(action.action_sequence, 1);
        assert_eq!(action.selection_token.as_deref(), Some("example:ctx-1"));
        assert_eq!(action.fields.len(), 1);

        let unknown = br#"{"protocol":3,"message":"view_action","view_instance_id":"a:b","view_revision":1,"action_id":"a:b","action_sequence":1,"price":9}"#;
        assert!(decode_loader_view_client_request(unknown).is_err());
        let wrong_protocol =
            br#"{"protocol":2,"message":"view_request","request_kind":"example:showcase"}"#;
        assert!(matches!(
            decode_loader_view_client_request(wrong_protocol),
            Err(LoaderHandshakeError::Protocol { .. })
        ));
        let request =
            br#"{"protocol":3,"message":"view_request","request_kind":"example:showcase"}"#;
        assert_eq!(
            decode_loader_view_client_request(request).unwrap(),
            LoaderViewClientRequest::ViewRequest {
                request_kind: mc_script::ScriptClientViewKey::try_new("example:showcase").unwrap(),
            }
        );
        for invalid_key in [
            "showcase",
            "example:",
            "example:Showcase",
            "other namespace:panel",
        ] {
            let payload = serde_json::to_vec(&serde_json::json!({
                "protocol": 3,
                "message": "view_request",
                "request_kind": invalid_key,
            }))
            .unwrap();
            assert!(matches!(
                decode_loader_view_client_request(&payload),
                Err(LoaderHandshakeError::Malformed(_))
            ));
        }
        let cancel = br#"{"protocol":3,"message":"cancel_selection","selection_context_id":"example:ctx-1"}"#;
        assert_eq!(
            decode_loader_view_client_request(cancel).unwrap(),
            LoaderViewClientRequest::CancelSelection {
                selection_context_id: "example:ctx-1".to_owned(),
            }
        );
    }
}

#[cfg(test)]
mod index_tests;
