use super::*;

fn rendered(text: &str) -> (f64, usize) {
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 240).unwrap();
    let height = {
        let cr = cairo::Context::new(&surface).unwrap();
        render_markdown(&cr, text, 10.0, 10.0, 380.0, 14.0)
    };
    surface.flush();
    let width = surface.width() as usize;
    let rows = surface.height() as usize;
    let stride = surface.stride() as usize;
    let data = surface.data().unwrap();
    let visible = data
        .chunks(stride)
        .take(rows)
        .flat_map(|row| row[..width * 4].chunks_exact(4))
        .filter(|pixel| pixel.iter().any(|byte| *byte != 0))
        .count();
    (height, visible)
}

#[test]
fn empty_markdown_has_zero_height() {
    let (height, visible) = rendered("");
    assert_eq!(height, 0.0);
    assert_eq!(visible, 0);
}

#[test]
fn plain_markdown_draws_visible_text() {
    let (height, visible) = rendered("plain text");
    assert!(height > 0.0, "plain text has positive height: {height}");
    assert!(visible > 0, "plain text leaves visible pixels");
}

#[test]
fn styled_markdown_and_hard_break_draw_multiple_lines() {
    let (one_line_height, _) = rendered("**strong** *emphasis* `inline code` next line");
    let (height, visible) = rendered("**strong**\n*emphasis* `inline code`  \nnext line");
    assert!(visible > 0, "styled markdown leaves visible pixels");
    assert!(
        height > one_line_height,
        "hard break adds height: {height} > {one_line_height}"
    );
}
