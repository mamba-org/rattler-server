use crate::cli::{self, Solver};
use crate::error::ApiError;
use anyhow::Context;
use rattler_conda_types::{Channel, Platform, RepoDataRecord};
use rattler_networking::AuthenticatedClient;
use rattler_repodata_gateway::fetch;
use reqwest::Url;
use std::sync::Arc;
use std::time::Duration;
use std::{default::Default, path::PathBuf};
use tracing::{span, Instrument, Level};

use crate::generic_cache::{GenericCache, GetCachedResult};

pub enum RepoData {
    Libsolvc(LibsolvcRepoData),
    Resolvo(ResolvoRepoData),
}

/// Caches the available packages for (channel, platform) pairs
pub struct AvailablePackagesCache {
    cache: GenericCache<Url, RepoData>,
    cache_dir: PathBuf,
    download_client: AuthenticatedClient,
}

impl AvailablePackagesCache {
    /// Creates an empty `AvailablePackagesCache` with keys that expire after `expiration`
    pub fn new(expiration: Duration, cache_dir: PathBuf) -> AvailablePackagesCache {
        AvailablePackagesCache {
            cache: GenericCache::with_expiration(expiration),
            download_client: AuthenticatedClient::default(),
            cache_dir,
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
        solver: cli::Solver,
    ) -> Result<Arc<RepoData>, ApiError> {
        let platform_url = channel.platform_url(platform);
        let write_token = match self.cache.get_cached(&platform_url).await {
            GetCachedResult::Found(repodata) => return Ok(repodata),
            GetCachedResult::NotFound(write_guard) => write_guard,
        };

        // Download
        let result = fetch::fetch_repo_data(
            channel.platform_url(platform),
            self.download_client.clone(),
            self.cache_dir.clone(),
            fetch::FetchRepoDataOptions {
                ..Default::default()
            },
            None,
        )
        .instrument(span!(Level::DEBUG, "fetch_repo_data"))
        .await
        .map_err(|err| ApiError::FetchRepoDataJson(channel.platform_url(platform), err))?;

        let some_crap = rattler_conda_types::RepoData::from_path(&result.repo_data_json_path);
        let records = some_crap
            .context("loading repo data")
            .map_err(ApiError::Internal)?
            .into_repo_data_records(channel);

        let repodata = match solver {
            Solver::Resolvo => RepoData::Resolvo(ResolvoRepoData { records }),
            Solver::Libsolvc => tokio::task::spawn_blocking(move || {
                let solv_file = rattler_solve::libsolv_c::cache_repodata(
                    platform_url.to_string(),
                    records.as_slice(),
                );
                RepoData::Libsolvc(LibsolvcRepoData { records, solv_file })
            })
            .instrument(span!(Level::DEBUG, "cache_libsolv_repodata"))
            .await
            .context("panicked while creating .solv file")
            .map_err(ApiError::Internal)?,
        };
        let repodata = Arc::new(repodata);

        // Update the cache
        self.cache.set(write_token, repodata.clone());
        Result::Ok(repodata)
    }
}

/// Owned counterpart to `resolvo::RepoData`
pub struct ResolvoRepoData {
    records: Vec<RepoDataRecord>,
}

impl ResolvoRepoData {
    pub fn as_repo_data(&self) -> rattler_solve::resolvo::RepoData {
        self.records.iter().collect()
    }
}

/// Owned counterpart to `libsolvc::RepoData`
pub struct LibsolvcRepoData {
    records: Vec<RepoDataRecord>,
    solv_file: rattler_solve::libsolv_c::LibcByteSlice,
}

impl LibsolvcRepoData {
    /// Returns a [`libsolv_c::RepoData`], borrowed from this instance
    pub fn as_repo_data(&self) -> rattler_solve::libsolv_c::RepoData {
        rattler_solve::libsolv_c::RepoData {
            records: self.records.iter().collect(),
            solv_file: todo!(),
        }
    }
}
