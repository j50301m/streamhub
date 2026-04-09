use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum TranscoderError {
    #[error("ffmpeg failed: {0}")]
    FfmpegFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Transcode an MP4 file to HLS (m3u8 + ts segments) in the given output directory.
/// Returns the path to the generated m3u8 playlist.
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
