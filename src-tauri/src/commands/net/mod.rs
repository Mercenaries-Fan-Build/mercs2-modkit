//! One transport layer for everything modkit downloads.
//!
//! Before this module there were **seven** `reqwest::Client` builders and **five**
//! implementations of "GET the latest GitHub release and pick an asset out of it" —
//! in [`super::setup`], [`super::dxwrapper`], [`super::toolchain`],
//! [`super::installer`], [`super::updates`], [`super::registry`] and
//! [`super::mercsink`]. They disagreed about everything that matters: none but
//! mercs.ink's set a timeout or honoured `Retry-After`, the tag fallback was
//! variously `"latest"`, an error, or the empty string, and the same failure
//! produced a different sentence in each caller.
//!
//! # Selection is by rule, never by a hardcoded filename
//!
//! [`release::AssetRule`] exists because of a specific, expensive failure. modkit
//! asked the pmc-blackbox release for an asset literally named `pmc_bb.dll`; that
//! release now publishes six feature-named variants and no such file, so the
//! installer could only report "No matching asset". The repair that looks obvious —
//! swap in the new spelling — is the same mistake one release later, and in that
//! case it silently installs a DLL with the ASI loader compiled out.
//!
//! So a caller states *which asset it wants and why*, in priority order, and gets a
//! single error naming what it asked for when nothing matches. What an artifact
//! **is** is [`super::managed`]'s problem; getting bytes for it is this module's.

pub mod archive;
pub mod client;
pub mod download;
pub mod release;

pub use client::{client, USER_AGENT};
pub use download::{download, DownloadOpts};
pub use release::{
    highest_satisfying, latest_release, list_releases, tag_version, Asset, AssetRule, Release,
    ReleaseHost,
};
