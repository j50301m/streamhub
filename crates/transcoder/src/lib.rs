//! Media processing helpers: local ffmpeg wrappers (HLS transcode, thumbnail
//! extraction, live HLS snapshot, MP4 concat) and a GCP Transcoder API client
//! for multi-resolution ABR HLS jobs.
#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Errors from transcoder operations.
#[derive(Debug, thiserror::Error)]
pub enum TranscoderError {
    /// ffmpeg exited non-zero. The payload is its stderr output.
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    /// Local filesystem or process IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// GCP Transcoder API returned a non-success response.
    #[error("API error: {0}")]
    Api(String),
    /// HTTP transport error when calling the Transcoder API.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

/// Transcodes (stream-copies) an MP4 into HLS (`index.m3u8` + `seg_NNN.ts`)
/// inside `output_dir` using the local `ffmpeg` binary. Returns the path to
/// the generated playlist.
///
/// # Errors
/// Returns [`TranscoderError::FfmpegFailed`] if `ffmpeg` exits non-zero, or
/// [`TranscoderError::Io`] on filesystem errors.
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

/// Extracts a single thumbnail frame from `input_mp4` and writes it to
/// `output_jpg` using `ffmpeg`.
///
/// # Errors
/// Returns [`TranscoderError::FfmpegFailed`] if `ffmpeg` exits non-zero, or
/// [`TranscoderError::Io`] on filesystem errors.
#[tracing::instrument]
pub async fn extract_thumbnail(
    input_mp4: &Path,
    output_jpg: &Path,
) -> Result<PathBuf, TranscoderError> {
    if let Some(parent) = output_jpg.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tracing::info!(
        input = %input_mp4.display(),
        output = %output_jpg.display(),
        "Extracting thumbnail from video"
    );

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            input_mp4.to_str().unwrap_or_default(),
            "-frames:v",
            "1",
            "-q:v",
            "5",
            output_jpg.to_str().unwrap_or_default(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::error!(%stderr, "ffmpeg thumbnail extraction failed");
        return Err(TranscoderError::FfmpegFailed(stderr));
    }

    tracing::info!(output = %output_jpg.display(), "Thumbnail extraction completed");
    Ok(output_jpg.to_path_buf())
}

/// Captures one frame from the live HLS stream at `hls_url` and writes it to
/// `output_path`. Used by the live-thumbnail task.
///
/// # Errors
/// Returns [`TranscoderError::FfmpegFailed`] if `ffmpeg` exits non-zero, or
/// [`TranscoderError::Io`] on filesystem errors.
#[tracing::instrument]
pub async fn capture_hls_thumbnail(
    hls_url: &str,
    output_path: &Path,
) -> Result<PathBuf, TranscoderError> {
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    tracing::info!(
        %hls_url,
        output = %output_path.display(),
        "Capturing thumbnail from HLS stream"
    );

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            hls_url,
            "-frames:v",
            "1",
            "-q:v",
            "5",
            output_path.to_str().unwrap_or_default(),
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        tracing::error!(%stderr, "ffmpeg HLS thumbnail capture failed");
        return Err(TranscoderError::FfmpegFailed(stderr));
    }

    tracing::info!(output = %output_path.display(), "HLS thumbnail capture completed");
    Ok(output_path.to_path_buf())
}

/// Concatenates `input_files` into a single MP4 at `output_path` using the
/// `ffmpeg` concat demuxer. A single-file input is returned as-is without
/// invoking ffmpeg.
///
/// # Errors
/// Returns [`TranscoderError::FfmpegFailed`] if `ffmpeg` exits non-zero, or
/// [`TranscoderError::Io`] on filesystem errors.
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

/// Submits a GCP Transcoder API job to transcode `input_uri` into
/// multi-resolution ABR HLS at `output_uri`. The `stream_id` is attached as
/// a job label for traceability.
///
/// Returns the job resource name
/// (e.g. `projects/{p}/locations/{l}/jobs/{id}`).
///
/// # Errors
/// Returns [`TranscoderError::Api`] on a non-success HTTP response, or
/// [`TranscoderError::Http`] on transport failure.
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
            "spriteSheets": [{
                "filePrefix": "thumb",
                "spriteWidthPixels": 640,
                "spriteHeightPixels": 360,
                "columnCount": 1,
                "rowCount": 1,
                "totalCount": 1,
                "quality": 80,
                "format": "jpeg"
            }],
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

/// Obtains a GCP access token by shelling out to `gcloud auth
/// print-access-token`. Suitable for local dev; production should use
/// Workload Identity instead.
///
/// # Errors
/// Returns [`TranscoderError::Api`] if `gcloud` exits non-zero, or
/// [`TranscoderError::Io`] if the binary cannot be spawned.
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
