use super::*;
use ab_glyph::{Font, FontArc, PxScale};
use image::{
    Rgba, RgbaImage,
    imageops::{self, FilterType},
};
use imageproc::drawing::{draw_text_mut, text_size};
const COLORS: [[u8; 4]; 7] = [
    [252, 92, 92, 255],
    [255, 147, 74, 255],
    [224, 162, 243, 255],
    [101, 200, 106, 255],
    [51, 199, 193, 255],
    [84, 156, 246, 255],
    [232, 108, 166, 255],
];

impl Bot {
    pub(super) async fn quote(&self, msg: &Value) -> Result<()> {
        let original = &msg["reply_to_message"];
        anyhow::ensure!(
            !original.is_null() && !moderation::text(original).is_empty(),
            "Ответьте /q на текстовое сообщение"
        );
        let user = id(&msg["from"]["id"]);
        let key = format!("{}{}", id(&msg["chat"]["id"]), id(&original["message_id"]));
        let config = self.app.config.read()?;
        let allowed = config::root(&config, user, self.app.settings.owner)
            || list(&self.app.requests().read()?["allowed_downloads"])
                .iter()
                .any(|v| v == &key);
        if allowed {
            self.deliver_quote(user, original).await?;
            self.reply(msg, "Стикер отправлен Вам в личные сообщения.")
                .await?;
        } else {
            self.request(
                "quote",
                json!({"user":user,"key":key,"message":original}),
                &config,
                "Запрос на создание стикера-цитаты",
            )
            .await?;
            self.reply(msg, "Запрос на создание цитаты отправлен администратору.")
                .await?;
        }
        Ok(())
    }
    pub(super) async fn deliver_quote(&self, user: i64, original: &Value) -> Result<()> {
        let author = original["author_signature"]
            .as_str()
            .or(original["from"]["first_name"].as_str())
            .unwrap_or("Anonymous");
        let author_id = id(&original["from"]["id"]);
        let avatar = self.quote_avatar(author_id).await;
        let bytes = render_quote(moderation::text(original), author, author_id, avatar)?;
        self.tg
            .upload("sendSticker", user, "sticker", "quote.webp", bytes)
            .await?;
        Ok(())
    }
    async fn quote_avatar(&self, user: i64) -> Option<RgbaImage> {
        let mut file = None;
        if user != 0 {
            if let Ok(photos) = self
                .tg
                .call("getUserProfilePhotos", json!({"user_id":user,"limit":1}))
                .await
            {
                file = list(&photos["photos"][0])
                    .last()
                    .and_then(|p| p["file_id"].as_str())
                    .map(str::to_owned);
            }
        }
        if file.is_none() {
            if let Ok(chat) = self
                .tg
                .call("getChat", json!({"chat_id":self.app.settings.main_channel}))
                .await
            {
                file = chat["photo"]["big_file_id"].as_str().map(str::to_owned);
            }
        }
        let bytes = self.tg.file(file.as_deref()?).await.ok()?;
        image::load_from_memory(&bytes).ok().map(|i| i.to_rgba8())
    }
}
fn wrap(text: &str, font: &FontArc, scale: PxScale, width: u32) -> Vec<String> {
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_inclusive(char::is_whitespace) {
            if !line.is_empty() && text_size(scale, font, &format!("{line}{word}")).0 > width {
                result.push(line.trim_end().into());
                line.clear();
            }
            for ch in word.chars() {
                if text_size(scale, font, &format!("{line}{ch}")).0 > width && !line.is_empty() {
                    result.push(line);
                    line = String::new();
                }
                line.push(ch);
            }
        }
        result.push(line.trim_end().into());
    }
    result
}
pub(super) fn render_quote(
    text: &str,
    author: &str,
    user: i64,
    avatar: Option<RgbaImage>,
) -> Result<Vec<u8>> {
    let font = FontArc::try_from_vec(std::fs::read("assets/fonts/Roboto-Regular.ttf")?)
        .map_err(|e| anyhow::anyhow!("Invalid font: {e}"))?;
    let bold = FontArc::try_from_vec(std::fs::read("assets/fonts/Roboto-Bold.ttf")?)
        .map_err(|e| anyhow::anyhow!("Invalid font: {e}"))?;
    // The Python renderer removed emoji. Unsupported glyphs are omitted here as well.
    let clean = |value: &str| {
        value
            .chars()
            .filter(|&c| font.glyph_id(c).0 != 0 || c == '\n')
            .collect::<String>()
    };
    let author = clean(author);
    let text = clean(text);
    let text_scale = PxScale::from(44.0);
    let name_scale = PxScale::from(48.0);
    let mut names = wrap(&author, &bold, name_scale, 840);
    names.truncate(4);
    let mut lines = wrap(&text, &font, text_scale, 840);
    lines.truncate(180);
    let width = names
        .iter()
        .map(|l| text_size(name_scale, &bold, l).0)
        .chain(lines.iter().map(|l| text_size(text_scale, &font, l).0))
        .max()
        .unwrap_or(100)
        .max(100)
        + 40;
    let bubble_height = 60 + names.len() as u32 * 58 + lines.len() as u32 * 54;
    let mut canvas = RgbaImage::new(120 + 100 + 10 + width, 120 + bubble_height.max(100));
    let mut bubble = RgbaImage::from_pixel(width, bubble_height, Rgba([27, 37, 50, 255]));
    let radius = 40i64;
    for y in 0..bubble_height {
        for x in 0..width {
            let dx = (radius - x as i64)
                .max(x as i64 - (width as i64 - 1 - radius))
                .max(0);
            let dy = (radius - y as i64)
                .max(y as i64 - (bubble_height as i64 - 1 - radius))
                .max(0);
            if dx * dx + dy * dy > radius * radius {
                bubble.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }
    imageops::overlay(&mut canvas, &bubble, 170, 60);
    let color = Rgba(COLORS[user.rem_euclid(7) as usize]);
    let mut y = 80;
    for line in &names {
        draw_text_mut(&mut canvas, color, 190, y, name_scale, &bold, line);
        y += 58;
    }
    y += 10;
    for line in &lines {
        draw_text_mut(
            &mut canvas,
            Rgba([255, 255, 255, 255]),
            190,
            y,
            text_scale,
            &font,
            line,
        );
        y += 54;
    }
    let mut avatar = if let Some(a) = avatar {
        let side = a.width().min(a.height());
        let crop = imageops::crop_imm(
            &a,
            (a.width() - side) / 2,
            (a.height() - side) / 2,
            side,
            side,
        )
        .to_image();
        imageops::resize(&crop, 100, 100, FilterType::Lanczos3)
    } else {
        let mut a = RgbaImage::from_pixel(100, 100, color);
        let letter = author
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        let scale = PxScale::from(58.0);
        let (w, h) = text_size(scale, &bold, &letter);
        draw_text_mut(
            &mut a,
            Rgba([255, 255, 255, 255]),
            (100 - w as i32) / 2,
            (100 - h as i32) / 2,
            scale,
            &bold,
            &letter,
        );
        a
    };
    for y in 0..100 {
        for x in 0..100 {
            if (x as i32 - 50).pow(2) + (y as i32 - 50).pow(2) > 2500 {
                avatar.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }
    imageops::overlay(&mut canvas, &avatar, 60, 60);
    let scale = 512.0 / canvas.width().max(canvas.height()) as f64;
    let canvas = imageops::resize(
        &canvas,
        ((canvas.width() as f64 * scale).round() as u32).max(1),
        ((canvas.height() as f64 * scale).round() as u32).max(1),
        FilterType::Lanczos3,
    );
    let mut bytes = std::io::Cursor::new(Vec::new());
    canvas.write_to(&mut bytes, image::ImageFormat::WebP)?;
    Ok(bytes.into_inner())
}
