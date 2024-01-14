use crate::error::ApiError;
use anyhow::Context;
use rattler_conda_types::{Channel, Platform, RepoData, RepoDataRecord};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::fetch;
use reqwest::Url;
use std::default::Default;
use std::sync::Arc;
use std::time::Duration;

use crate::generic_cache::{GenericCache, GetCachedResult};

/// Caches the available packages for (channel, platform) pairs
pub struct AvailablePackagesCache {
    cache: GenericCache<Url, Vec<RepoDataRecord>>,
    download_client: AuthenticatedClient,
}

impl AvailablePackagesCache {
    /// Creates an empty `AvailablePackagesCache` with keys that expire after `expiration`
    pub fn with_expiration(expiration: Duration) -> AvailablePackagesCache {
        AvailablePackagesCache {
            cache: GenericCache::with_expiration(expiration),
            download_client: AuthenticatedClient::default(),
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
    ) -> Result<Vec<RepoDataRecord>, ApiError> {
        let platform_url = channel.platform_url(platform);
        let write_token = match self.cache.get_cached(&platform_url).await {
            GetCachedResult::Found(repodata) => return Ok(repodata.to_vec()),
            GetCachedResult::NotFound(write_guard) => write_guard,
        };
        let cache_dir = dirs::cache_dir()
            .context("reading cache directory")
            .map_err(ApiError::Internal)?
            .join("rattler/cache");

        // Download
        let result = fetch::fetch_repo_data(
            channel.platform_url(platform),
            self.download_client.clone(),
            cache_dir,
            fetch::FetchRepoDataOptions {
                ..Default::default()
            },
            None,
        )
        .await
        .map_err(|err| ApiError::FetchRepoDataJson(channel.platform_url(platform), err))?;

        let repodata = RepoData::from_path(result.repo_data_json_path)
            .context("loading repo data")
            .map_err(ApiError::Internal)?
            .into_repo_data_records(channel);

        // Update the cache
        self.cache.set(write_token, Arc::new(repodata.clone()));
        Result::Ok(repodata)
    }
}
