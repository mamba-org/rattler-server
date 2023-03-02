use rattler_conda_types::{Channel, Platform, RepoDataRecord};
use rattler_solve::{cache_libsolv_repodata, LibcByteSlice, LibsolvRepoData};
use reqwest::{Client, Url};
use std::sync::Arc;

use crate::generic_cache::{GenericCache, GetCachedResult};

/// Caches the available packages for (channel, platform) pairs
pub struct AvailablePackagesCache {
    cache: GenericCache<Url, LibsolvOwnedRepoData>,
    download_client: Client,
}

impl AvailablePackagesCache {
    /// Creates an empty `AvailablePackagesCache`
    pub fn new() -> AvailablePackagesCache {
        AvailablePackagesCache {
            cache: GenericCache::new(),
            download_client: Client::new(),
        }
    }

    /// Gets the repo data for this channel and platform if they exist in the cache, and downloads
    /// them otherwise
    pub async fn get(
        &self,
        channel: &Channel,
        platform: Platform,
    ) -> Result<Arc<LibsolvOwnedRepoData>, String> {
        let platform_url = channel.platform_url(platform);
        let write_guard = match self.cache.get_cached(&platform_url).await {
            GetCachedResult::Found(repodata) => return Ok(repodata),
            GetCachedResult::NotFound(write_guard) => write_guard,
        };

        println!("Cache miss: {platform_url}");

        // Download
        let download_start = std::time::Instant::now();
        let records = crate::fetch::get_repodata(
            &self.download_client,
            channel,
            channel.platform_url(platform),
        )
        .await?;
        let download_end = std::time::Instant::now();
        println!(
            "Download and parse repodata.json: {} ms",
            (download_end - download_start).as_millis()
        );

        // Create .solv
        let create_solv_start = std::time::Instant::now();
        let solv_file = cache_libsolv_repodata(platform_url.to_string(), records.as_slice());
        let create_solv_end = std::time::Instant::now();
        println!(
            "Create .solv file: {} ms",
            (create_solv_end - create_solv_start).as_millis()
        );

        // Update the cache
        let owned_repodata = Arc::new(LibsolvOwnedRepoData { records, solv_file });
        self.cache
            .set(platform_url.clone(), owned_repodata.clone(), write_guard);

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
