// Miniaturas das telas/janelas na escolha do que transmitir (JPEG em data URL).
use base64::Engine;

use crate::encoder::RawFrame;

/// Largura das miniaturas (16:9 → 480x270)
pub const WIDTH: usize = 480;

pub fn to_data_url(frame: &RawFrame) -> anyhow::Result<String> {
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 72).encode(
        &frame.bgra,
        frame.width as u16,
        frame.height as u16,
        jpeg_encoder::ColorType::Bgra,
    )?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpeg)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gera_jpeg_em_data_url() {
        let frame = RawFrame { width: 480, height: 270, bgra: vec![128; 480 * 270 * 4] };
        let url = to_data_url(&frame).unwrap();
        assert!(url.starts_with("data:image/jpeg;base64,/9j/"));
    }
}
