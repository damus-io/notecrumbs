use crate::Error;
use bytes::Bytes;
use egui::{Color32, ColorImage};
use hyper::body::Incoming;
use image::imageops::FilterType;

pub const PFP_SIZE: u32 = 64;

// Thank to gossip for this one!
pub fn round_image(image: &mut ColorImage) {
    // The radius to the edge of of the avatar circle
    let edge_radius = image.size[0] as f32 / 2.0;
    let edge_radius_squared = edge_radius * edge_radius;

    for (pixnum, pixel) in image.pixels.iter_mut().enumerate() {
        // y coordinate
        let uy = pixnum / image.size[0];
        let y = uy as f32;
        let y_offset = edge_radius - y;

        // x coordinate
        let ux = pixnum % image.size[0];
        let x = ux as f32;
        let x_offset = edge_radius - x;

        // The radius to this pixel (may be inside or outside the circle)
        let pixel_radius_squared: f32 = x_offset * x_offset + y_offset * y_offset;

        // If inside of the avatar circle
        if pixel_radius_squared <= edge_radius_squared {
            // squareroot to find how many pixels we are from the edge
            let pixel_radius: f32 = pixel_radius_squared.sqrt();
            let distance = edge_radius - pixel_radius;

            // If we are within 1 pixel of the edge, we should fade, to
            // antialias the edge of the circle. 1 pixel from the edge should
            // be 100% of the original color, and right on the edge should be
            // 0% of the original color.
            if distance <= 1.0 {
                *pixel = Color32::from_rgba_premultiplied(
                    (pixel.r() as f32 * distance) as u8,
                    (pixel.g() as f32 * distance) as u8,
                    (pixel.b() as f32 * distance) as u8,
                    (pixel.a() as f32 * distance) as u8,
                );
            }
        } else {
            // Outside of the avatar circle
            *pixel = Color32::TRANSPARENT;
        }
    }
}

pub fn process_pfp_bitmap(image: &mut image::DynamicImage) -> ColorImage {
    let size = PFP_SIZE;

    // Crop square
    let smaller = image.width().min(image.height());

    if image.width() > smaller {
        let excess = image.width() - smaller;
        *image = image.crop_imm(excess / 2, 0, image.width() - excess, image.height());
    } else if image.height() > smaller {
        let excess = image.height() - smaller;
        *image = image.crop_imm(0, excess / 2, image.width(), image.height() - excess);
    }
    let image = image.resize(size, size, FilterType::CatmullRom); // DynamicImage
    let image_buffer = image.into_rgba8(); // RgbaImage (ImageBuffer)
    let mut color_image = ColorImage::from_rgba_unmultiplied(
        [
            image_buffer.width() as usize,
            image_buffer.height() as usize,
        ],
        image_buffer.as_flat_samples().as_slice(),
    );
    round_image(&mut color_image);
    color_image
}

async fn _fetch_url(url: &str) -> Result<(Vec<u8>, hyper::Response<Incoming>), Error> {
    use http_body_util::BodyExt;
    use http_body_util::Empty;
    use hyper::Request;
    use hyper_util::rt::tokio::TokioIo;
    use tokio::net::TcpStream;

    let mut data: Vec<u8> = vec![];
    let url = url.parse::<hyper::Uri>()?;
    let host = url.host().expect("uri has no host");
    let port = url.port_u16().unwrap_or(80);
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(addr).await?;
    let io = TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::handshake(io).await?;
    tokio::task::spawn(async move {
        if let Err(err) = conn.await {
            println!("Connection failed: {:?}", err);
        }
    });

    let authority = url.authority().unwrap().clone();

    let req = Request::builder()
        .uri(url)
        .header(hyper::header::HOST, authority.as_str())
        .body(Empty::<Bytes>::new())?;

    let mut res: hyper::Response<Incoming> = sender.send_request(req).await?;

    // Stream the body, writing each chunk to stdout as we get it
    // (instead of buffering and printing at the end).
    while let Some(next) = res.frame().await {
        let frame = next?;
        if let Some(chunk) = frame.data_ref() {
            if data.len() + chunk.len() > 52428800
            /* 50 MiB */
            {
                return Err(Error::TooBig);
            }
            data.extend(chunk);
        }
    }

    Ok((data, res))
}

pub async fn _fetch_pfp(url: &str) -> Result<ColorImage, Error> {
    let (data, res) = _fetch_url(url).await?;
    _parse_img_response(data, res)
}

fn _parse_img_response(
    data: Vec<u8>,
    response: hyper::Response<Incoming>,
) -> Result<ColorImage, Error> {
    use egui_extras::image::FitTo;

    let content_type = response.headers()["content-type"]
        .to_str()
        .unwrap_or_default();

    let size = PFP_SIZE;

    if content_type.starts_with("image/svg") {
        let mut color_image =
            egui_extras::image::load_svg_bytes_with_size(&data, FitTo::Size(size, size))?;
        round_image(&mut color_image);
        Ok(color_image)
    } else if content_type.starts_with("image/") {
        let mut dyn_image = image::load_from_memory(&data)?;
        Ok(process_pfp_bitmap(&mut dyn_image))
    } else {
        Err(Error::InvalidProfilePic)
    }
}
