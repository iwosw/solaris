//! Network edge for core model v2 (ADR 0013).
//!
//! I/O threads own sockets and connection state; they decode packets off-tick
//! into [`mc_core::boundary::PlayerInput`] and submit them to the core, then
//! fan out each [`mc_core::boundary::TickOutput`] per viewer in parallel.
//! The edge never reads or writes core state except through the boundary.
//!
//! Entity interest uses the sorted chunk index. Publish shares dimension-local
//! change buckets; viewers retain their window and planning buffers. Movement
//! follows the vanilla type's id-phased update interval and horizontal tracking
//! square, capped by view distance and gated on the client holding the chunk.
//! A due phase crossed by coalesced snapshots is reconciled on the next observed
//! tick, so a viewer cannot permanently miss an entity's update phase.
//! Tracked baselines advance only when the whole packet group fits world credit;
//! deferred moves, appearance changes and removals are reconciled later.
//!
//! Slow-consumer policy (see [`fanout`]): per-connection bounded outbound
//! queues; fanout uses `try_send` and never blocks the tick. On a full queue
//! best-effort packets (chunk data, entity moves) are coalesced to the latest
//! and older ones dropped; reliable packets (disconnect, block ack, inventory,
//! health) are retained up to the bound, then the connection is dropped rather
//! than stalling the tick.

pub use mc_core::boundary;

mod admin;
pub mod advancements;
pub mod anti_xray;
pub mod backup;
mod boss_bars;
pub mod chunks;
mod command_tree;
pub mod computer;
pub mod config;
pub mod configuration;
pub mod connection;
mod console;
pub mod content_cache;
pub mod content_import;
pub mod crypto;
pub mod dashboard;
pub mod decode;
mod dh;
mod dimensions;
pub mod fanout;
pub mod handshake;
mod light;
mod lod;
pub mod login;
pub mod msmp;
pub mod operator_cli;
pub mod plan;
pub mod play;
mod plugin_loader;
pub mod proxy;
pub mod recipes;
pub mod server;
pub(crate) mod settlement_labels;
pub mod startup_data;
pub mod status;

mod conn_limit;
mod vanilla_items;
mod server_properties;
mod resource_pack;
mod remote_admin;

#[cfg(test)]
mod plugin_tests;

#[cfg(test)]
mod plugin_command_tree_tests;

#[cfg(test)]
mod settlement_command_tree_tests;

#[cfg(test)]
mod plugin_wire_tests;
