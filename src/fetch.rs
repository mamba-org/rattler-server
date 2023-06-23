use crate::error::ApiError;
use futures::TryStreamExt;
use rattler_conda_types::{Channel, RepoData, RepoDataRecord};
use rattler_networking::AuthenticatedClient;
use reqwest::{Response, Url};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;
use tracing::{span, Instrument, Level};

// Download and parse `repodata.json`
#[tracing::instrument(level = Level::DEBUG, skip(client))]
pub async fn get_repodata(
    client: &AuthenticatedClient,
    channel: &Channel,
    platform_url: Url,
) -> Result<Vec<RepoDataRecord>, ApiError> {
    let (repodata_url, encoding) = get_repodata_url(client, &platform_url).await;
    let repodata_url_clone = repodata_url.clone();
    let response = client
        .get(repodata_url)
        .send()
        .await
        .map_err(|e| ApiError::FetchRepoDataJson(repodata_url_clone.clone(), e))?
        .error_for_status()
        .map_err(|e| ApiError::FetchRepoDataJson(repodata_url_clone, e))?;
    let records = stream_and_decode_to_memory(response, encoding, channel.clone())
        .await
        .map_err(ApiError::Internal)?;
    Ok(records)
}

#[tracing::instrument(level = Level::DEBUG, skip_all)]
async fn stream_and_decode_to_memory(
    response: Response,
    encoding: Option<Encoding>,
    channel: Channel,
) -> anyhow::Result<Vec<RepoDataRecord>> {
    let bytes = response
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

    let mut json_bytes = Vec::new();
    async {
        match encoding {
            None => {
                StreamReader::new(bytes)
                    .read_to_end(&mut json_bytes)
                    .await?;
            }
            Some(Encoding::Bz2) => {
                async_compression::tokio::bufread::BzDecoder::new(StreamReader::new(bytes))
                    .read_to_end(&mut json_bytes)
                    .await?;
            }
            Some(Encoding::Zst) => {
                async_compression::tokio::bufread::ZstdDecoder::new(StreamReader::new(bytes))
                    .read_to_end(&mut json_bytes)
                    .await?;
            }
        };

        Ok::<(), anyhow::Error>(())
    }
    .instrument(span!(Level::DEBUG, "download repodata.json"))
    .await?;

    tokio::task::spawn_blocking(move || {
        let repodata: RepoData = serde_json::from_slice(&json_bytes)?;
        Ok(repodata.into_repo_data_records(&channel))
    })
    .instrument(span!(Level::DEBUG, "parse repodata.json"))
    .await?
}

enum Encoding {
    Zst,
    Bz2,
}

#[tracing::instrument(level = Level::DEBUG, skip(client))]
async fn get_repodata_url(
    client: &AuthenticatedClient,
    subdir_url: &Url,
) -> (Url, Option<Encoding>) {
    let variant = rattler_repodata_gateway::fetch::Variant::AfterPatches;
    let variant_availability = rattler_repodata_gateway::fetch::check_variant_availability(
        client,
        subdir_url,
        None,
        variant.file_name(),
    )
    .await;

    let has_zst = variant_availability.has_zst();
    let has_bz2 = variant_availability.has_bz2();

    if has_zst {
        let url = subdir_url
            .join("repodata.json.zst")
            .expect("invalid url segment");
        (url, Some(Encoding::Zst))
    } else if has_bz2 {
        let url = subdir_url
            .join("repodata.json.bz2")
            .expect("invalid url segment");
        (url, Some(Encoding::Bz2))
    } else {
        let url = subdir_url
            .join("repodata.json")
            .expect("invalid url segment");
        (url, None)
    }
}
