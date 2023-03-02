use futures::TryStreamExt;
use rattler_conda_types::{Channel, RepoData, RepoDataRecord};
use std::time::Instant;
use reqwest::{Client, Response, Url};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;

// Download and parse `repodata.json`
pub async fn get_repodata(
    client: &Client,
    channel: &Channel,
    platform_url: Url,
) -> Result<Vec<RepoDataRecord>, String> {
    let (repodata_url, encoding) = get_repodata_url(client, &platform_url).await;

    println!("Downloading from url: {repodata_url}");

    // TODO: proper error handling
    let response = client
        .get(repodata_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let records = stream_and_decode_to_memory(response, encoding, channel.clone()).await;
    Ok(records)
}

async fn stream_and_decode_to_memory(
    response: Response,
    encoding: Option<Encoding>,
    channel: Channel,
) -> Vec<RepoDataRecord> {
    let start_read = Instant::now();

    let bytes = response
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));

    let mut json_bytes = Vec::new();
    match encoding {
        None => {
            StreamReader::new(bytes)
                .read_to_end(&mut json_bytes)
                .await
                .unwrap();
        }
        Some(Encoding::Bz2) => {
            async_compression::tokio::bufread::BzDecoder::new(StreamReader::new(bytes))
                .read_to_end(&mut json_bytes)
                .await
                .unwrap();
        }
        Some(Encoding::Zst) => {
            async_compression::tokio::bufread::ZstdDecoder::new(StreamReader::new(bytes))
                .read_to_end(&mut json_bytes)
                .await
                .unwrap();
        }
    };

    let end_read = Instant::now();

    let start_parse = Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let repodata: RepoData = serde_json::from_slice(&json_bytes).unwrap();
        repodata.into_repo_data_records(&channel)
    })
    .await
    .unwrap();

    let end_parse = Instant::now();

    println!(
        "Stream repodata.json bytes: {} ms",
        (end_read - start_read).as_millis()
    );
    println!(
        "Parse repodata.json: {} ms",
        (end_parse - start_parse).as_millis()
    );

    result
}

enum Encoding {
    Zst,
    Bz2,
}

async fn get_repodata_url(client: &Client, subdir_url: &Url) -> (Url, Option<Encoding>) {
    let variant_availability =
        rattler_repodata_gateway::fetch::check_variant_availability(client, subdir_url, None).await;

    let has_zst = variant_availability.has_zst();
    let has_bz2 = variant_availability.has_bz2();

    if has_zst {
        (
            subdir_url.join("repodata.json.zst").unwrap(),
            Some(Encoding::Zst),
        )
    } else if has_bz2 {
        (
            subdir_url.join("repodata.json.bz2").unwrap(),
            Some(Encoding::Bz2),
        )
    } else {
        (subdir_url.join("repodata.json").unwrap(), None)
    }
}
