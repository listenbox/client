use crate::{api::Api, publicapi as p};
use anyhow::{Context, Result, bail, ensure};
use std::io::Cursor;

const MAX_BYTES: usize = 20 << 20;

pub(crate) async fn upload_source(api: &Api, show: &str, url: &str) -> Result<String> {
    let response = api.send(api.http.get(url)).await?;
    ensure!(
        response.status().is_success(),
        "download show artwork: HTTP {}",
        response.status()
    );
    let raw = api.bytes(response, MAX_BYTES).await?;
    upload(api, show, raw).await
}

/// The client transfers image bytes directly to storage; the public API owns
/// allocation and validation. No source image or temporary file is retained.
pub async fn upload(api: &Api, show: &str, raw: Vec<u8>) -> Result<String> {
    ensure!(
        !raw.is_empty() && raw.len() <= MAX_BYTES,
        "artwork must contain at most 20 MiB"
    );
    let (raw, content_type) = api
        .wait(async {
            tokio::task::spawn_blocking(move || -> Result<_> {
                let mut reader =
                    image::ImageReader::new(Cursor::new(&raw)).with_guessed_format()?;
                match reader.format() {
                    Some(image::ImageFormat::Jpeg) => Ok((raw, "image/jpeg")),
                    Some(image::ImageFormat::Png) => Ok((raw, "image/png")),
                    Some(image::ImageFormat::WebP) => {
                        let mut limits = image::Limits::default();
                        limits.max_alloc = Some(64 << 20);
                        reader.limits(limits);
                        let image = reader.decode().context("decode YouTube artwork")?;
                        let mut png = Cursor::new(Vec::new());
                        image.write_to(&mut png, image::ImageFormat::Png)?;
                        Ok((png.into_inner(), "image/png"))
                    }
                    _ => bail!("artwork must be a JPEG, PNG or WebP image"),
                }
            })
            .await?
        })
        .await?;
    ensure!(raw.len() <= MAX_BYTES, "converted artwork exceeds 20 MiB");
    let signed = match api
        .client()
        .create_image_upload_presign(p::CreateImageUploadPresignParams {
            body: p::CreateImageUploadPresign {
                show_id: show.into(),
                content_type: serde_json::from_value(serde_json::json!(content_type))?,
                byte_length: raw.len() as i64,
                file_name: None,
            },
        })
        .await?
    {
        p::CreateImageUploadPresignResponse::Status201(value) => value,
        response => return Err(api.response_error(response).await),
    };
    let response = api
        .send(
            api.http
                .put(&signed.upload_url)
                .header("Content-Type", content_type)
                .body(raw),
        )
        .await?;
    ensure!(
        response.status().is_success(),
        "upload artwork: HTTP {}",
        response.status()
    );
    let completed = match api
        .client()
        .complete_image_upload(p::CompleteImageUploadParams {
            image_asset_id: signed.image_asset_id,
            body: p::CompleteImageUpload {
                object_key: signed.object_key,
            },
        })
        .await?
    {
        p::CompleteImageUploadResponse::Status201(value) => value,
        response => return Err(api.response_error(response).await),
    };
    Ok(completed.id)
}
