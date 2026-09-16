//! Host renderer for inspecting the same Markdown reader pipeline used by T1.

use prs_markdown::harness::{render_page, HostReader};
use prs_markdown::layout::Viewport;
use prs_markdown::style::{Insets, ReaderStyle, TextStyle};
use prs_markdown::typography::{FontConfig, FontdueTextEngine};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_OUTPUT: &str = "target/prs-markdown-pages";

#[derive(Debug)]
struct Cli {
    input: Input,
    output: PathBuf,
    pages: Vec<usize>,
    font_regular: Option<PathBuf>,
    font_bold: Option<PathBuf>,
    font_italic: Option<PathBuf>,
    font_bold_italic: Option<PathBuf>,
    font_monospace: Option<PathBuf>,
    width: u32,
    height: u32,
    padding: u32,
    body_size: u32,
    line_height: u32,
    heading_size: u32,
    heading_line_height: u32,
    code_size: u32,
    code_line_height: u32,
    cache_capacity: usize,
}

#[derive(Debug)]
enum Input {
    File(PathBuf),
    Fixture(String),
}

#[derive(Debug)]
struct FontPaths {
    regular: PathBuf,
    bold: PathBuf,
    italic: PathBuf,
    bold_italic: PathBuf,
    monospace: PathBuf,
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let Some(cli) = Cli::parse(env::args().skip(1))? else {
        print_usage();
        return Ok(());
    };

    let (source_path, source) = read_input(&cli.input)?;
    let (font_paths, font_config) = load_font_config(&cli)?;
    let font_engine = FontdueTextEngine::new(font_config, cli.cache_capacity)?;
    let style = reader_style(&cli);
    let viewport = Viewport::new(cli.width, cli.height);
    // Clone the configured engine so layout metrics and rasterization use the
    // same Fontdue faces and settings. The renderer retains its own cache.
    let reader =
        HostReader::from_source_with_measurer(&source, style, viewport, font_engine.clone())?;
    let mut renderer = prs_markdown::EmbeddedGraphicsRenderer::new(font_engine);

    let page_indices = selected_pages(&cli.pages, reader.page_count())?;
    fs::create_dir_all(&cli.output)
        .map_err(|error| format!("create output directory {}: {error}", cli.output.display()))?;

    println!("input: {}", source_path.display());
    println!("font regular: {}", font_paths.regular.display());
    println!("font bold: {}", font_paths.bold.display());
    println!("font italic: {}", font_paths.italic.display());
    println!("font bold-italic: {}", font_paths.bold_italic.display());
    println!("font monospace: {}", font_paths.monospace.display());
    println!("viewport: {}x{}", viewport.width, viewport.height);
    println!("pages: {}", reader.page_count());
    println!("output: {}", cli.output.display());

    for index in page_indices {
        let page = reader.page(index).expect("validated page index");
        let image = render_page(page, &mut renderer);
        let pgm_output = cli.output.join(format!("page-{:03}.pgm", page.number));
        let png_output = cli.output.join(format!("page-{:03}.png", page.number));
        image.save_pgm(&pgm_output)?;
        image.save_png(&png_output)?;
        let visible = reader.visible_fragments(index).unwrap_or_default();
        println!(
            "page {}: range {:?} commands={} hit_regions={} visible_fragments={} pgm={} png={}",
            page.number,
            page.logical_range(),
            page.display_list().len(),
            page.hit_regions.len(),
            visible.len(),
            pgm_output.display(),
            png_output.display()
        );
    }

    Ok(())
}

impl Cli {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Self>, String> {
        let mut input = None;
        let mut output = PathBuf::from(DEFAULT_OUTPUT);
        let mut pages = Vec::new();
        let mut font_regular = None;
        let mut font_bold = None;
        let mut font_italic = None;
        let mut font_bold_italic = None;
        let mut font_monospace = None;
        let mut width = 600;
        let mut height = 800;
        let mut padding = 16;
        let mut body_size = 16;
        let mut line_height = 22;
        let mut heading_size = 22;
        let mut heading_line_height = 28;
        let mut code_size = 14;
        let mut code_line_height = 20;
        let mut cache_capacity = 512;

        while let Some(argument) = args.next() {
            match argument.as_str() {
                "-h" | "--help" => return Ok(None),
                "--fixture" => {
                    let name = required_value(&mut args, "--fixture")?;
                    if input.is_some() {
                        return Err("choose one input file or fixture".into());
                    }
                    input = Some(Input::Fixture(name));
                }
                "--output" | "-o" => output = PathBuf::from(required_value(&mut args, "--output")?),
                "--page" => pages.push(parse_page(&required_value(&mut args, "--page")?)?),
                "--font" | "--font-regular" => {
                    font_regular = Some(PathBuf::from(required_value(&mut args, "--font")?))
                }
                "--font-bold" => {
                    font_bold = Some(PathBuf::from(required_value(&mut args, "--font-bold")?))
                }
                "--font-italic" => {
                    font_italic = Some(PathBuf::from(required_value(&mut args, "--font-italic")?))
                }
                "--font-bold-italic" => {
                    font_bold_italic = Some(PathBuf::from(required_value(
                        &mut args,
                        "--font-bold-italic",
                    )?))
                }
                "--font-monospace" => {
                    font_monospace = Some(PathBuf::from(required_value(
                        &mut args,
                        "--font-monospace",
                    )?))
                }
                "--width" => width = parse_u32(&required_value(&mut args, "--width")?, "--width")?,
                "--height" => {
                    height = parse_u32(&required_value(&mut args, "--height")?, "--height")?
                }
                "--padding" => {
                    padding = parse_u32(&required_value(&mut args, "--padding")?, "--padding")?
                }
                "--body-size" => {
                    body_size =
                        parse_u32(&required_value(&mut args, "--body-size")?, "--body-size")?
                }
                "--line-height" => {
                    line_height = parse_u32(
                        &required_value(&mut args, "--line-height")?,
                        "--line-height",
                    )?
                }
                "--heading-size" => {
                    heading_size = parse_u32(
                        &required_value(&mut args, "--heading-size")?,
                        "--heading-size",
                    )?
                }
                "--heading-line-height" => {
                    heading_line_height = parse_u32(
                        &required_value(&mut args, "--heading-line-height")?,
                        "--heading-line-height",
                    )?
                }
                "--code-size" => {
                    code_size =
                        parse_u32(&required_value(&mut args, "--code-size")?, "--code-size")?
                }
                "--code-line-height" => {
                    code_line_height = parse_u32(
                        &required_value(&mut args, "--code-line-height")?,
                        "--code-line-height",
                    )?
                }
                "--cache-capacity" => {
                    cache_capacity = required_value(&mut args, "--cache-capacity")?
                        .parse()
                        .map_err(|_| "--cache-capacity must be a non-negative integer")?
                }
                value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
                value => {
                    if input.is_some() {
                        return Err("choose one input file or fixture".into());
                    }
                    input = Some(Input::File(PathBuf::from(value)));
                }
            }
        }

        let input = input.ok_or_else(|| "provide a Markdown file or --fixture NAME".to_owned())?;
        Ok(Some(Self {
            input,
            output,
            pages,
            font_regular,
            font_bold,
            font_italic,
            font_bold_italic,
            font_monospace,
            width,
            height,
            padding,
            body_size,
            line_height,
            heading_size,
            heading_line_height,
            code_size,
            code_line_height,
            cache_capacity,
        }))
    }
}

fn load_font_config(cli: &Cli) -> Result<(FontPaths, FontConfig), Box<dyn Error>> {
    let regular = cli
        .font_regular
        .clone()
        .or_else(default_font_path)
        .ok_or_else(|| "no host font found; pass --font PATH".to_owned())?;
    let paths = FontPaths {
        regular: regular.clone(),
        bold: cli.font_bold.clone().unwrap_or_else(|| regular.clone()),
        italic: cli.font_italic.clone().unwrap_or_else(|| regular.clone()),
        bold_italic: cli
            .font_bold_italic
            .clone()
            .unwrap_or_else(|| regular.clone()),
        monospace: cli
            .font_monospace
            .clone()
            .unwrap_or_else(|| regular.clone()),
    };
    let regular_bytes = read_font(&paths.regular, "regular")?;
    let bold_bytes = read_font(&paths.bold, "bold")?;
    let italic_bytes = read_font(&paths.italic, "italic")?;
    let bold_italic_bytes = read_font(&paths.bold_italic, "bold-italic")?;
    let monospace_bytes = read_font(&paths.monospace, "monospace")?;
    let config = FontConfig::from_faces(
        regular_bytes,
        bold_bytes,
        italic_bytes,
        bold_italic_bytes,
        monospace_bytes,
    );
    Ok((paths, config))
}

fn read_font(path: &Path, face: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    fs::read(path).map_err(|error| format!("read {face} font {}: {error}", path.display()).into())
}

fn reader_style(cli: &Cli) -> ReaderStyle {
    ReaderStyle {
        body: TextStyle::new(cli.body_size.max(1), cli.line_height.max(1)),
        heading: TextStyle {
            bold: true,
            ..TextStyle::new(cli.heading_size.max(1), cli.heading_line_height.max(1))
        },
        code: TextStyle {
            code: true,
            ..TextStyle::new(cli.code_size.max(1), cli.code_line_height.max(1))
        },
        page_padding: Insets::all(cli.padding),
        ..ReaderStyle::default()
    }
}

fn read_input(input: &Input) -> Result<(PathBuf, String), Box<dyn Error>> {
    let path = match input {
        Input::File(path) => path.clone(),
        Input::Fixture(name) => fixture_path(name)?,
    };
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("read Markdown {}: {error}", path.display()))?;
    Ok((path, source))
}

fn fixture_path(name: &str) -> Result<PathBuf, String> {
    let name = name.strip_suffix(".md").unwrap_or(name);
    let path = Path::new(name);
    if name.is_empty() || path.components().count() != 1 || path.file_name().is_none() {
        return Err("fixture name must be a single Markdown fixture name".into());
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.md"));
    if !path.is_file() {
        return Err(format!("fixture does not exist: {name}"));
    }
    Ok(path)
}

fn selected_pages(requested: &[usize], page_count: usize) -> Result<Vec<usize>, String> {
    if requested.is_empty() {
        return Ok((0..page_count).collect());
    }
    let mut selected = Vec::with_capacity(requested.len());
    for &page in requested {
        if page == 0 || page > page_count {
            return Err(format!("page {page} is outside 1..={page_count}"));
        }
        let index = page - 1;
        if !selected.contains(&index) {
            selected.push(index);
        }
    }
    selected.sort_unstable();
    Ok(selected)
}

fn parse_page(value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| "--page must be a positive 1-based page number".to_owned())
}

fn parse_u32(value: &str, option: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("{option} must be a non-negative integer"))
}

fn required_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn default_font_path() -> Option<PathBuf> {
    env::var_os("PRS_MARKDOWN_FONT")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            [
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
                "/Library/Fonts/Arial.ttf",
                "C:\\Windows\\Fonts\\arial.ttf",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
        })
}

fn print_usage() {
    println!(
        "Usage: prs-markdown-harness [OPTIONS] FILE\n\n\
         Render the production Markdown parser/layout/pagination/renderer pipeline\n\
         into PGM and PNG files per selected page. With no --page, all pages are rendered.\n\n\
         Input:\n\
           FILE                         Markdown file to read\n\
           --fixture NAME               checked-in tests/fixtures/NAME.md\n\n\
         Output and selection:\n\
           -o, --output DIR             output directory (default: {DEFAULT_OUTPUT})\n\
           --page N                     render 1-based page N; repeat for selected pages\n\n\
         Configuration:\n\
           --font PATH                  regular face; unspecified faces use it (or PRS_MARKDOWN_FONT)\n\
           --font-regular PATH          alias for --font\n\
           --font-bold PATH             bold face\n\
           --font-italic PATH           italic face\n\
           --font-bold-italic PATH      bold-italic face\n\
           --font-monospace PATH        monospace/code face\n\
           --width N                    viewport width (default: 600)\n\
           --height N                   viewport height (default: 800)\n\
           --padding N                  page padding on all sides (default: 16)\n\
           --body-size N                body font size (default: 16)\n\
           --line-height N              body line height (default: 22)\n\
           --heading-size N             heading font size (default: 22)\n\
           --heading-line-height N      heading line height (default: 28)\n\
           --code-size N                code font size (default: 14)\n\
           --code-line-height N         code line height (default: 20)\n\
           --cache-capacity N           bounded glyph cache entries (default: 512)\n\
           -h, --help                   show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_independent_font_face_options() {
        let cli = Cli::parse(
            [
                "--fixture",
                "regression",
                "--font-regular",
                "regular.ttf",
                "--font-bold",
                "bold.ttf",
                "--font-italic",
                "italic.ttf",
                "--font-bold-italic",
                "bold-italic.ttf",
                "--font-monospace",
                "mono.ttf",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("font options should parse")
        .expect("arguments should produce a CLI");

        assert_eq!(cli.font_regular, Some(PathBuf::from("regular.ttf")));
        assert_eq!(cli.font_bold, Some(PathBuf::from("bold.ttf")));
        assert_eq!(cli.font_italic, Some(PathBuf::from("italic.ttf")));
        assert_eq!(cli.font_bold_italic, Some(PathBuf::from("bold-italic.ttf")));
        assert_eq!(cli.font_monospace, Some(PathBuf::from("mono.ttf")));
    }
}
