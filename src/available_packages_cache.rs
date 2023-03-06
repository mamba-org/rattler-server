use anyhow::Context;
use rattler_conda_types::{Channel, Platform, RepoDataRecord};
use rattler_solve::{cache_libsolv_repodata, LibcByteSlice, LibsolvRepoData};
use reqwest::{Client, Url};
use std::sync::Arc;
use std::time::Duration;
use tracing::{span, Instrument, Level};

use crate::generic_cache::{GenericCache, GetCachedResult};

/// Caches the available packages for (channel, platform) pairs
pub struct AvailablePackagesCache {
    cache: GenericCache<Url, LibsolvOwnedRepoData>,
    download_client: Client,
}

impl AvailablePackagesCache {
    /// Creates an empty `AvailablePackagesCache` with keys that expire after `expiration`
    pub fn with_expiration(expiration: Duration) -> AvailablePackagesCache {
        AvailablePackagesCache {
            cache: GenericCache::with_expiration(expiration),
            download_client: Client::new(),
        }
    }

    /// Removes outdated data from the cache
    pub fn gc(&self) {
        self.cache.gc();
    }

    /// Gets the repo data for this channel and platform if they exist in the cache, and downloads
    /// them otherwise
    pub async fn get(
        &self,
        channel: &Channel,
        platform: Platform,
    ) -> Result<Arc<LibsolvOwnedRepoData>, anyhow::Error> {
        let platform_url = channel.platform_url(platform);
        let write_token = match self.cache.get_cached(&platform_url).await {
            GetCachedResult::Found(repodata) => return Ok(repodata),
            GetCachedResult::NotFound(write_guard) => write_guard,
        };

        // Download
        let records = crate::fetch::get_repodata(
            &self.download_client,
            channel,
            channel.platform_url(platform),
        )
        .await?;

        // Create .solv (can block for seconds)
        let platform_url_clone = platform_url.clone();
        let owned_repodata = tokio::task::spawn_blocking(move || {
            let solv_file =
                cache_libsolv_repodata(platform_url_clone.to_string(), records.as_slice());
            Arc::new(LibsolvOwnedRepoData { records, solv_file })
        })
        .instrument(span!(Level::DEBUG, "cache_libsolv_repodata"))
        .await
        .context("panicked while creating .solv file")?;

        // Update the cache
        self.cache.set(write_token, owned_repodata.clone());

        Ok(owned_repodata)
    }
}

/// Owned counterpart to `LibsolvRepoData`
pub struct LibsolvOwnedRepoData {
    records: Vec<RepoDataRecord>,
    solv_file: LibcByteSlice,
}

impl LibsolvOwnedRepoData {
    /// Returns a [`LibsolvRepoData`], borrowed from this instance
    pub fn as_libsolv_repo_data(&self) -> LibsolvRepoData {
        LibsolvRepoData {
            records: self.records.as_slice(),
            solv_file: Some(&self.solv_file),
        }
    }
}
