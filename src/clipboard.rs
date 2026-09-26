//! Turn clipboard text, copied files, and screenshots into terminal input.
use anyhow::{Context, Result};
use arboard::{Clipboard, Error, ImageData};
use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_IMAGE: AtomicU64 = AtomicU64::new(0);

/// Search and picker fields accept only text; the terminal also accepts paths.
pub(crate) fn paste_text(clipboard: &mut Clipboard, allow_paths: bool) -> Result<Option<String>> {
    read(clipboard, allow_paths, &std::env::temp_dir())
}

trait Source {
    fn files(&mut self) -> Result<Vec<PathBuf>, Error>;
    fn text(&mut self) -> Result<String, Error>;
    fn image(&mut self) -> Result<ImageData<'static>, Error>;
}

impl Source for Clipboard {
    fn files(&mut self) -> Result<Vec<PathBuf>, Error> { self.get().file_list() }
    fn text(&mut self) -> Result<String, Error> { self.get_text() }
    fn image(&mut self) -> Result<ImageData<'static>, Error> { self.get_image() }
}

fn available<T>(result: Result<T, Error>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn read(source: &mut impl Source, allow_paths: bool, image_dir: &Path) -> Result<Option<String>> {
    // Finder also advertises plain text containing just the filenames. Prefer
    // the actual file URLs so a copied screenshot keeps its original path.
    if allow_paths {
        if let Some(paths) = available(source.files()).context("Could not read clipboard files")? {
            if !paths.is_empty() {
                let quoted: Option<Vec<_>> = paths.iter().map(|p| crate::input::quoted_path(p)).collect();
                let quoted = quoted.context("A clipboard path contains unsupported characters")?;
                log::debug!("pasting {} clipboard file paths", paths.len());
                return Ok(Some(quoted.concat()));
            }
        }
    }
    if let Some(text) = available(source.text()).context("Could not read clipboard text")? {
        if !text.is_empty() { return Ok(Some(text)); }
    }
    if allow_paths {
        if let Some(image) = available(source.image()).context("Could not read clipboard image")? {
            let quoted = save_image(&image, image_dir).context("Could not save clipboard image")?;
            return Ok(Some(quoted));
        }
    }
    Ok(None)
}

/// Keep successful files in the OS temporary directory, including after the
/// terminal exits, so a command can open the pasted path later. Never replace
/// an earlier paste or make a screenshot readable by other users.
fn save_image(image: &ImageData<'_>, directory: &Path) -> Result<String> {
    let width = u32::try_from(image.width).context("Clipboard image is too wide")?;
    let height = u32::try_from(image.height).context("Clipboard image is too tall")?;
    let expected = image.width.checked_mul(image.height).and_then(|n| n.checked_mul(4));
    anyhow::ensure!(width > 0 && height > 0 && expected == Some(image.bytes.len()),
        "Invalid clipboard image dimensions or pixel data");
    let directory = directory.canonicalize().context("Temporary directory is unavailable")?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    loop {
        let id = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("arabic-term-clipboard-{}-{timestamp}-{id}.png", std::process::id()));
        let quoted = crate::input::quoted_path(&path).context("Temporary path contains unsupported characters")?;
        let file = match options.open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("Could not create temporary PNG"),
        };
        if let Err(error) = write_png(file, image, width, height) {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        log::debug!("saved clipboard image ({width}x{height}) to {}", path.display());
        return Ok(quoted);
    }
}

fn write_png(file: File, image: &ImageData<'_>, width: u32, height: u32) -> Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.bytes)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let id = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!("arabic-term-clipboard-test-{timestamp}-{id} لقطة 'شاشة'"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn is_empty(&self) -> bool { std::fs::read_dir(&self.0).unwrap().next().is_none() }
    }
    impl Drop for Temp { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[derive(Default)]
    struct Fake {
        files: Option<Vec<PathBuf>>,
        text: Option<String>,
        image: Option<ImageData<'static>>,
        reads: Vec<&'static str>,
    }
    impl Source for Fake {
        fn files(&mut self) -> Result<Vec<PathBuf>, Error> {
            self.reads.push("files");
            self.files.clone().ok_or(Error::ContentNotAvailable)
        }
        fn text(&mut self) -> Result<String, Error> {
            self.reads.push("text");
            self.text.clone().ok_or(Error::ContentNotAvailable)
        }
        fn image(&mut self) -> Result<ImageData<'static>, Error> {
            self.reads.push("image");
            self.image.clone().ok_or(Error::ContentNotAvailable)
        }
    }

    fn screenshot() -> ImageData<'static> {
        ImageData { width: 2, height: 1, bytes: vec![255, 0, 0, 255, 0, 120, 255, 80].into() }
    }

    fn shell_paths(quoted: &str) -> Vec<PathBuf> {
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf '%s\\0' {quoted}")]).output().unwrap();
        assert!(output.status.success());
        output.stdout.split(|&b| b == 0).filter(|s| !s.is_empty())
            .map(|s| PathBuf::from(std::str::from_utf8(s).unwrap())).collect()
    }

    #[test]
    fn screenshot_paste_creates_a_complete_png_and_preserves_earlier_pastes() {
        let temp = Temp::new();
        let image = screenshot();
        let mut source = Fake { image: Some(image.clone()), ..Default::default() };
        let pasted = read(&mut source, true, &temp.0).unwrap().unwrap();
        let paths = shell_paths(&pasted);
        assert_eq!(paths.len(), 1);
        assert!(paths[0].is_absolute());
        assert_eq!(paths[0].extension().unwrap(), "png");
        let mut decoder = png::Decoder::new(BufReader::new(File::open(&paths[0]).unwrap())).read_info().unwrap();
        let mut pixels = vec![0; decoder.output_buffer_size().unwrap()];
        let info = decoder.next_frame(&mut pixels).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (2, 1, png::ColorType::Rgba));
        assert_eq!(&pixels[..info.buffer_size()], image.bytes.as_ref());
        assert_eq!(source.reads, ["files", "text", "image"]);
        let second = shell_paths(&read(&mut source, true, &temp.0).unwrap().unwrap());
        assert_ne!(paths, second);
        assert!(paths[0].exists() && second[0].exists());
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&paths[0]).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn copied_files_take_precedence_over_names_and_previews() {
        let temp = Temp::new();
        let files = vec![temp.0.join("لقطة 'screen' $(printf injected).png"), temp.0.join("screen two.png")];
        let mut source = Fake { files: Some(files.clone()), text: Some("screen.png".into()),
            image: Some(screenshot()), ..Default::default() };
        let pasted = read(&mut source, true, &temp.0).unwrap().unwrap();
        assert_eq!(shell_paths(&pasted), files);
        assert_eq!(source.reads, ["files"]);
        assert!(temp.is_empty());
    }

    #[test]
    fn text_keeps_its_contents_and_does_not_save_an_image() {
        let temp = Temp::new();
        let text = "echo 'مرحبا'\r\nsecond line";
        let mut source = Fake { text: Some(text.into()), image: Some(screenshot()), ..Default::default() };
        assert_eq!(read(&mut source, true, &temp.0).unwrap().as_deref(), Some(text));
        assert_eq!(source.reads, ["files", "text"]);
        assert!(temp.is_empty());
    }

    #[test]
    fn text_fields_do_not_read_files_or_save_screenshots() {
        let temp = Temp::new();
        let mut source = Fake { files: Some(vec![temp.0.join("screen.png")]), image: Some(screenshot()), ..Default::default() };
        assert!(read(&mut source, false, &temp.0).unwrap().is_none());
        assert_eq!(source.reads, ["text"]);
        source.text = Some("search query".into());
        assert_eq!(read(&mut source, false, &temp.0).unwrap().as_deref(), Some("search query"));
        assert!(temp.is_empty());
    }

    #[test]
    fn empty_formats_fall_through_to_the_image_or_report_no_content() {
        let temp = Temp::new();
        let mut source = Fake { files: Some(vec![]), text: Some(String::new()), ..Default::default() };
        assert!(read(&mut source, true, &temp.0).unwrap().is_none());
        assert!(temp.is_empty());
        source.image = Some(screenshot());
        let paths = shell_paths(&read(&mut source, true, &temp.0).unwrap().unwrap());
        assert!(paths[0].is_file());
    }

    #[test]
    fn failed_reads_and_writes_are_errors_instead_of_empty_clipboards() {
        assert!(available::<String>(Err(Error::ClipboardOccupied)).is_err());
        assert!(available::<String>(Err(Error::ConversionFailure)).is_err());
        let temp = Temp::new();
        let mut source = Fake { image: Some(screenshot()), ..Default::default() };
        let error = read(&mut source, true, &temp.0.join("missing")).unwrap_err();
        assert!(error.to_string().contains("Could not save clipboard image"));
        source.image.as_mut().unwrap().bytes = vec![0].into();
        assert!(read(&mut source, true, &temp.0).is_err());
        assert!(temp.is_empty());
    }

    #[test]
    fn unsupported_file_paths_fail_without_pasting_a_partial_list() {
        let temp = Temp::new();
        let mut source = Fake { files: Some(vec![temp.0.join("good.png"), temp.0.join("bad\npath.png")]),
            ..Default::default() };
        assert!(read(&mut source, true, &temp.0).is_err());
        assert!(temp.is_empty());
    }
}
