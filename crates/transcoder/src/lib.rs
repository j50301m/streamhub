use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum TranscoderError {
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("API error: {0}")]
    Api(String),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Transcode an MP4 file to HLS (m3u8 + ts segments) in the given output directory.
/// Returns the path to the generated m3u8 playlist.
#[tracing::instrument]
pub async fn transcode_to_hls(
    input_mp4: &Path,
    output_dir: &Path,
) -> Result<PathBuf, TranscoderError> {
    tokio::fs::create_dir_all(output_dir).await?;

    let output_m3u8 = output_dir.join("index.m3u8");
    let segment_pattern = output_dir.join("seg_%03d.ts");

    tracing::info!(
        input = %input_mp4.display(),
        output = %output_m3u8.display(),
        "Starting ffmpeg transcode to HLS"
    );

    let output = Command::new("ffmpeg")
        .args([
            "-i",
            input_mp4.to_str().unwrap_or_default(),
            "-c",
            "copy",
            "-hls_time",
            "6",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
            segment_pattern.to_str().unwrap_or_default(),
            output_m3u8.to_str().unwrap_or_default(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::error!(%stderr, "ffmpeg transcode failed");
        return Err(TranscoderError::FfmpegFailed(stderr));
    }

    tracing::info!(output = %output_m3u8.display(), "ffmpeg transcode completed");
    Ok(output_m3u8)
}

/// Concatenate multiple MP4 files into a single MP4 using ffmpeg concat demuxer.
/// Returns the path to the combined output file.
#[tracing::instrument(fields(files = input_files.len()))]
pub async fn concat_mp4(
    input_files: &[PathBuf],
    output_path: &Path,
) -> Result<PathBuf, TranscoderError> {
    if input_files.len() == 1 {
        // Single file, no concat needed — just return the path
        return Ok(input_files[0].clone());
    }

    // Write concat filelist
    let filelist_path = output_path
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join("concat_list.txt");
    let filelist_content: String = input_files
        .iter()
        .map(|f| format!("file '{}'\n", f.display()))
        .collect();
    tokio::fs::write(&filelist_path, &filelist_content).await?;

    tracing::info!(
        files = input_files.len(),
        output = %output_path.display(),
        "Concatenating MP4 files"
    );

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            filelist_path.to_str().unwrap_or_default(),
            "-c",
            "copy",
            output_path.to_str().unwrap_or_default(),
        ])
        .output()
        .await?;

    // Clean up filelist
    let _ = tokio::fs::remove_file(&filelist_path).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::error!(%stderr, "ffmpeg concat failed");
        return Err(TranscoderError::FfmpegFailed(stderr));
    }

    tracing::info!(output = %output_path.display(), "MP4 concat completed");
    Ok(output_path.to_path_buf())
}

/// Create a GCP Transcoder API job to transcode an MP4 into multi-resolution ABR HLS.
///
/// Returns the job name (e.g. `projects/{p}/locations/{l}/jobs/{id}`).
#[tracing::instrument(skip(auth_token))]
pub async fn create_job(
    project_id: &str,
    location: &str,
    input_uri: &str,
    output_uri: &str,
    stream_id: &str,
    auth_token: &str,
) -> Result<String, TranscoderError> {
    let url = format!(
        "https://transcoder.googleapis.com/v1/projects/{project_id}/locations/{location}/jobs"
    );

    let body = serde_json::json!({
        "inputUri": input_uri,
        "outputUri": output_uri,
        "config": {
            "elementaryStreams": [
                {
                    "key": "video-1080p",
                    "videoStream": {
                        "h264": {
                            "widthPixels": 1920,
                            "heightPixels": 1080,
                            "bitrateBps": 5_000_000
                        }
                    }
                },
                {
                    "key": "video-720p",
                    "videoStream": {
                        "h264": {
                            "widthPixels": 1280,
                            "heightPixels": 720,
                            "bitrateBps": 2_500_000
                        }
                    }
                },
                {
                    "key": "video-360p",
                    "videoStream": {
                        "h264": {
                            "widthPixels": 640,
                            "heightPixels": 360,
                            "bitrateBps": 1_000_000
                        }
                    }
                },
                {
                    "key": "audio",
                    "audioStream": {
                        "codec": "aac",
                        "bitrateBps": 128_000
                    }
                }
            ],
            "muxStreams": [
                {
                    "key": "hls-1080p",
                    "container": "ts",
                    "elementaryStreams": ["video-1080p", "audio"],
                    "segmentSettings": { "segmentDuration": "6s" }
                },
                {
                    "key": "hls-720p",
                    "container": "ts",
                    "elementaryStreams": ["video-720p", "audio"],
                    "segmentSettings": { "segmentDuration": "6s" }
                },
                {
                    "key": "hls-360p",
                    "container": "ts",
                    "elementaryStreams": ["video-360p", "audio"],
                    "segmentSettings": { "segmentDuration": "6s" }
                }
            ],
            "manifests": [
                {
                    "fileName": "index.m3u8",
                    "type": "HLS",
                    "muxStreams": ["hls-1080p", "hls-720p", "hls-360p"]
                }
            ],
            "pubsubDestination": {
                "topic": format!("projects/{project_id}/topics/streamhub-transcoder")
            }
        },
        "labels": {
            "stream_id": stream_id
        }
    });

    tracing::info!(
        %input_uri,
        %output_uri,
        %stream_id,
        "Creating GCP Transcoder API job"
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {auth_token}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(%status, %body, "Transcoder API request failed");
        return Err(TranscoderError::Api(format!("{status}: {body}")));
    }

    let resp_body: serde_json::Value = resp.json().await?;
    let job_name = resp_body["name"].as_str().unwrap_or_default().to_string();

    tracing::info!(%job_name, "Transcoder job created");
    Ok(job_name)
}

/// Obtain a GCP access token using `gcloud auth print-access-token`.
pub async fn get_gcp_token() -> Result<String, TranscoderError> {
    let output = Command::new("gcloud")
        .args(["auth", "print-access-token"])
        .output()
        .await?;

    if !output.status.success() {
        return Err(TranscoderError::Api(
            "gcloud auth print-access-token failed".to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
