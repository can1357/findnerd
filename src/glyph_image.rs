//! Large glyph previews: rasterizes a codepoint from an installed Nerd Font and
//! hands the pixels to whichever terminal graphics protocol is available.

use std::{
	env, fs,
	sync::mpsc::{self, Receiver, TryRecvError},
	thread,
};

use ab_glyph::{Font, FontVec, PxScale};
use image::{DynamicImage, Rgb, RgbImage};
use ratatui::{Frame, layout::Rect, style::Color};
use ratatui_image::{
	Image, Resize,
	picker::{Picker, ProtocolType},
	protocol::Protocol,
};

/// Path to a font file that takes precedence over system font discovery.
const FONT_ENV: &str = "FINDNERD_FONT";
/// Families tried first; any other family containing "Nerd Font" follows.
const PREFERRED_FAMILIES: [&str; 2] = ["Symbols Nerd Font", "Symbols Nerd Font Mono"];
const MAX_FONTS: usize = 3;
/// Share of the preview square covered by the glyph's longest side.
const GLYPH_FILL: f32 = 0.8;

/// Renders the selected glyph as an image in the preview panel.
pub struct GlyphPreview {
	picker: Picker,
	fonts: Fonts,
	current: Option<(PreviewKey, Protocol)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct PreviewKey {
	glyph: char,
	area: Rect,
}

enum Fonts {
	Loading(Receiver<Vec<FontVec>>),
	Ready(Vec<FontVec>),
}

impl GlyphPreview {
	/// Queries the terminal for a pixel graphics protocol and starts loading
	/// fonts in the background. Must run inside the alternate screen, before
	/// the event loop starts reading input.
	///
	/// Returns `None` when the terminal can only draw half blocks, which are
	/// too coarse for icon detail.
	pub fn detect() -> Option<Self> {
		let picker = Picker::from_query_stdio().ok()?;
		if picker.protocol_type() == ProtocolType::Halfblocks {
			return None;
		}
		let (sender, receiver) = mpsc::channel();
		thread::spawn(move || {
			sender.send(load_fonts()).ok();
		});
		Some(Self { picker, fonts: Fonts::Loading(receiver), current: None })
	}

	/// Draws `glyph` centered in `area`. Returns `false` when it could not be
	/// rasterized (fonts still loading or no font covers it), so the caller can
	/// fall back to text.
	pub fn render(
		&mut self,
		frame: &mut Frame<'_>,
		area: Rect,
		glyph: char,
		fg: Color,
		bg: Color,
	) -> bool {
		let Some(fonts) = self.fonts.ready() else {
			return false;
		};
		let (cell_width, cell_height) = self.picker.font_size();
		let Some(target) = square_cells(area, cell_width, cell_height) else {
			return false;
		};
		let key = PreviewKey { glyph, area: target };
		if self
			.current
			.as_ref()
			.is_none_or(|(current, _)| *current != key)
		{
			let width = u32::from(target.width) * u32::from(cell_width);
			let height = u32::from(target.height) * u32::from(cell_height);
			let Some(pixels) = rasterize(fonts, glyph, width, height, rgb(fg), rgb(bg)) else {
				self.current = None;
				return false;
			};
			let Ok(protocol) =
				self
					.picker
					.new_protocol(DynamicImage::ImageRgb8(pixels), target, Resize::Fit(None))
			else {
				self.current = None;
				return false;
			};
			self.current = Some((key, protocol));
		}
		let Some((_, protocol)) = &self.current else {
			return false;
		};
		frame.render_widget(Image::new(protocol), target);
		true
	}
}

impl Fonts {
	fn ready(&mut self) -> Option<&[FontVec]> {
		if let Self::Loading(receiver) = self {
			match receiver.try_recv() {
				Ok(fonts) => *self = Self::Ready(fonts),
				Err(TryRecvError::Empty) => return None,
				Err(TryRecvError::Disconnected) => *self = Self::Ready(Vec::new()),
			}
		}
		match self {
			Self::Ready(fonts) => Some(fonts),
			Self::Loading(_) => None,
		}
	}
}

fn load_fonts() -> Vec<FontVec> {
	let mut fonts = Vec::new();
	if let Some(font) = env::var_os(FONT_ENV)
		.and_then(|path| fs::read(path).ok())
		.and_then(|data| FontVec::try_from_vec(data).ok())
	{
		fonts.push(font);
	}

	let mut database = fontdb::Database::new();
	database.load_system_fonts();
	let mut faces = database
		.faces()
		.filter_map(|face| {
			let rank = face
				.families
				.iter()
				.filter_map(|(family, _)| family_rank(family))
				.min()?;
			Some((rank, face.id))
		})
		.collect::<Vec<_>>();
	faces.sort_by_key(|(rank, _)| *rank);
	for (_, id) in faces {
		if fonts.len() >= MAX_FONTS {
			break;
		}
		let font = database
			.with_face_data(id, |data, index| {
				FontVec::try_from_vec_and_index(data.to_vec(), index).ok()
			})
			.flatten();
		fonts.extend(font);
	}
	fonts
}

fn family_rank(family: &str) -> Option<usize> {
	PREFERRED_FAMILIES
		.iter()
		.position(|preferred| *preferred == family)
		.or_else(|| {
			family
				.contains("Nerd Font")
				.then_some(PREFERRED_FAMILIES.len())
		})
}

/// The largest centered rect inside `area` whose pixel size is roughly square.
fn square_cells(area: Rect, cell_width: u16, cell_height: u16) -> Option<Rect> {
	if area.width == 0 || area.height == 0 || cell_width == 0 || cell_height == 0 {
		return None;
	}
	let (cell_width, cell_height) = (u32::from(cell_width), u32::from(cell_height));
	let mut height = u32::from(area.height);
	let mut width = height * cell_height / cell_width;
	if width > u32::from(area.width) {
		width = u32::from(area.width);
		height = (width * cell_width / cell_height).max(1);
	}
	let (width, height) = (width as u16, height as u16);
	Some(Rect {
		x: area.x + (area.width - width) / 2,
		y: area.y + (area.height - height) / 2,
		width,
		height,
	})
}

/// Draws `glyph` scaled to fill [`GLYPH_FILL`] of the shorter side, centered
/// on an opaque background.
fn rasterize(
	fonts: &[FontVec],
	glyph: char,
	width: u32,
	height: u32,
	fg: [u8; 3],
	bg: [u8; 3],
) -> Option<RgbImage> {
	let font = fonts.iter().find(|font| font.glyph_id(glyph).0 != 0)?;
	let id = font.glyph_id(glyph);
	// Measure at one pixel per font unit; `Outline::bounds` is y-flipped, so its
	// height comes out negative and would let tall glyphs overflow.
	let unit_bounds = font
		.outline_glyph(id.with_scale(font.height_unscaled()))?
		.px_bounds();
	let extent = unit_bounds.width().max(unit_bounds.height());
	if extent <= 0.0 {
		return None;
	}
	let target = width.min(height) as f32 * GLYPH_FILL;
	let scale = PxScale::from(font.height_unscaled() * target / extent);
	let outlined = font.outline_glyph(id.with_scale(scale))?;
	let glyph_bounds = outlined.px_bounds();
	let left = ((width as f32 - glyph_bounds.width()) / 2.0).round() as i64;
	let top = ((height as f32 - glyph_bounds.height()) / 2.0).round() as i64;

	let mut image = RgbImage::from_pixel(width, height, Rgb(bg));
	outlined.draw(|x, y, coverage| {
		let (Ok(x), Ok(y)) = (u32::try_from(left + i64::from(x)), u32::try_from(top + i64::from(y)))
		else {
			return;
		};
		if x >= width || y >= height {
			return;
		}
		let coverage = coverage.clamp(0.0, 1.0);
		let blend = |back: u8, front: u8| {
			(f32::from(front) - f32::from(back))
				.mul_add(coverage, f32::from(back))
				.round() as u8
		};
		image.put_pixel(x, y, Rgb([blend(bg[0], fg[0]), blend(bg[1], fg[1]), blend(bg[2], fg[2])]));
	});
	Some(image)
}

const fn rgb(color: Color) -> [u8; 3] {
	match color {
		Color::Rgb(red, green, blue) => [red, green, blue],
		Color::Black => [0, 0, 0],
		_ => [255, 255, 255],
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn square_cells_compensates_for_tall_cells() {
		let area = Rect::new(0, 0, 40, 10);
		let square = square_cells(area, 10, 20).expect("non-empty area");
		assert_eq!((square.width, square.height), (20, 10));
		assert_eq!((square.x, square.y), (10, 0));
	}

	#[test]
	fn square_cells_shrinks_to_narrow_areas() {
		let area = Rect::new(0, 0, 8, 10);
		let square = square_cells(area, 10, 20).expect("non-empty area");
		assert_eq!((square.width, square.height), (8, 4));
		assert_eq!(square.y, 3);
	}

	#[test]
	fn tall_glyphs_stay_inside_the_image() {
		let fonts = load_fonts();
		// linux-forgejo is taller than it is wide; skipped without a Nerd Font.
		let Some(image) = rasterize(&fonts, '\u{f335}', 100, 100, [255; 3], [0; 3]) else {
			return;
		};
		let edge = (0..100).flat_map(|i| [(i, 0), (i, 99), (0, i), (99, i)]);
		assert!(edge.into_iter().all(|(x, y)| image.get_pixel(x, y).0 == [0; 3]));
	}

	#[test]
	fn nerd_symbol_families_rank_first() {
		assert_eq!(family_rank("Symbols Nerd Font"), Some(0));
		assert_eq!(family_rank("CaskaydiaMono Nerd Font"), Some(2));
		assert_eq!(family_rank("DejaVu Sans"), None);
	}
}


