//! Command-line parsing (std only).

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage:
  rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--quality <1-100>] [--cpu]
              [--profile <name>] [--lens <name>] [--lens-zoom <factor>] [--no-lens]
  rne info <file>
  rne lenses [<words>...]
  rne profiles <file>
  rne gpu
  rne help

develop   Develops a RAW file to an sRGB JPEG. Without -o the JPEG goes next to
          the RAW file; an existing file is never overwritten.
          Lens corrections (lensfun) are applied when camera and lens are known.
          --profile    camera profile by name (see `rne profiles`); default: the
                       camera's built-in matrices
          --lens       lens by its database name, when the file does not say it
                       or says it wrongly (see `rne lenses`)
          --lens-zoom  magnification after correction (default: no empty borders)
          --no-lens    no lens corrections
info      Shows camera, lens, shooting data and the corrections that would apply.
lenses    Lists the lenses of the lens database whose name contains every word.
profiles  Lists the camera profiles (DCP) installed for the camera of a RAW file.
gpu       Shows the GPU that would be used.";

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Develop(DevelopArgs),
    Info(PathBuf),
    Lenses(Vec<String>),
    Profiles(PathBuf),
    Gpu,
    Help,
}

/// Arguments of `develop`.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopArgs {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub exposure: Option<f32>,
    pub quality: u8,
    pub cpu_only: bool,
    /// Camera profile by name; `None` for the built-in matrices.
    pub profile: Option<String>,
    /// Lens chosen by name.
    pub lens: Option<String>,
    pub lens_zoom: Option<f32>,
    pub no_lens: bool,
}

/// Default JPEG quality: high enough to keep full colour resolution.
const DEFAULT_QUALITY: u8 = 92;

/// Parses the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut it = args.into_iter();
    let Some(cmd) = it.next() else { return Ok(Command::Help) };
    match cmd.as_str() {
        "help" | "-h" | "--help" => Ok(Command::Help),
        "gpu" => no_more(it).map(|()| Command::Gpu),
        "info" => {
            let file = it.next().ok_or("info needs a file")?;
            no_more(it)?;
            Ok(Command::Info(PathBuf::from(file)))
        }
        "develop" => parse_develop(it).map(Command::Develop),
        "lenses" => Ok(Command::Lenses(it.collect())),
        "profiles" => {
            let file = it.next().ok_or("profiles needs a RAW file")?;
            no_more(it)?;
            Ok(Command::Profiles(PathBuf::from(file)))
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn no_more(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    it.next().map_or(Ok(()), |extra| Err(format!("unexpected argument '{extra}'")))
}

fn parse_develop(mut it: impl Iterator<Item = String>) -> Result<DevelopArgs, String> {
    let mut input = None;
    let mut a = DevelopArgs {
        input: PathBuf::new(),
        output: None,
        exposure: None,
        quality: DEFAULT_QUALITY,
        cpu_only: false,
        profile: None,
        lens: None,
        lens_zoom: None,
        no_lens: false,
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-o" | "--output" => a.output = Some(PathBuf::from(value("-o")?)),
            "--exposure" => {
                let v = value("--exposure")?;
                // Accept a decimal comma as well as a point.
                let stops: f32 = v.replace(',', ".").parse().map_err(|_| format!("'{v}' is not a number of stops"))?;
                if !stops.is_finite() {
                    return Err(format!("'{v}' is not a number of stops"));
                }
                a.exposure = Some(stops);
            }
            "--quality" => {
                let v = value("--quality")?;
                a.quality = v
                    .parse()
                    .ok()
                    .filter(|q| (1..=100).contains(q))
                    .ok_or_else(|| format!("quality must be 1-100, not '{v}'"))?;
            }
            "--cpu" => a.cpu_only = true,
            "--profile" => {
                let v = value("--profile")?;
                a.profile = (!v.eq_ignore_ascii_case("builtin") && !v.eq_ignore_ascii_case("built-in")).then_some(v);
            }
            "--lens" => a.lens = Some(value("--lens")?),
            "--lens-zoom" => {
                let v = value("--lens-zoom")?;
                a.lens_zoom = Some(
                    v.replace(',', ".")
                        .parse::<f32>()
                        .ok()
                        .filter(|z| z.is_finite() && (0.25..=4.0).contains(z))
                        .ok_or_else(|| format!("lens zoom must be 0.25-4, not '{v}'"))?,
                );
            }
            "--no-lens" => a.no_lens = true,
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            file if input.is_none() => input = Some(PathBuf::from(file)),
            extra => return Err(format!("unexpected argument '{extra}'")),
        }
    }
    a.input = input.ok_or("develop needs a RAW file")?;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Command, String> {
        parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn develop_with_every_option() {
        let c = parse_str("develop a.dng -o b.jpg --exposure 0,5 --quality 80 --cpu").expect("parses");
        assert_eq!(
            c,
            Command::Develop(DevelopArgs {
                input: PathBuf::from("a.dng"),
                output: Some(PathBuf::from("b.jpg")),
                exposure: Some(0.5),
                quality: 80,
                cpu_only: true,
                profile: None,
                lens: None,
                lens_zoom: None,
                no_lens: false,
            })
        );
    }

    #[test]
    fn profile_and_lens_options() {
        let args =
            ["develop", "a.arw", "--profile", "Adobe Standard", "--lens", "Sony FE 50mm f/1.8", "--lens-zoom", "1,05"];
        let Ok(Command::Develop(a)) = parse(args.map(str::to_owned)) else { panic!("expected develop") };
        assert_eq!(a.profile.as_deref(), Some("Adobe Standard"));
        assert_eq!(a.lens.as_deref(), Some("Sony FE 50mm f/1.8"));
        assert_eq!(a.lens_zoom, Some(1.05));
        let Ok(Command::Develop(a)) = parse_str("develop a.arw --profile builtin --no-lens") else { panic!("develop") };
        assert_eq!((a.profile, a.no_lens), (None, true));
        assert_eq!(parse_str("develop a.arw --lens-zoom 9"), Err("lens zoom must be 0.25-4, not '9'".to_owned()));
        assert_eq!(parse_str("lenses sony 50mm"), Ok(Command::Lenses(vec!["sony".to_owned(), "50mm".to_owned()])));
        assert_eq!(parse_str("profiles a.nef"), Ok(Command::Profiles(PathBuf::from("a.nef"))));
        assert_eq!(parse_str("profiles"), Err("profiles needs a RAW file".to_owned()));
    }

    #[test]
    fn develop_defaults() {
        let Ok(Command::Develop(a)) = parse_str("develop a.arw") else { panic!("expected develop") };
        assert_eq!((a.output, a.exposure, a.quality, a.cpu_only), (None, None, 92, false));
    }

    #[test]
    fn negative_exposure_is_a_value_not_an_option() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --exposure -1.5") else { panic!("expected develop") };
        assert_eq!(a.exposure, Some(-1.5));
    }

    #[test]
    fn mistakes_are_explained() {
        assert_eq!(parse_str("develop"), Err("develop needs a RAW file".to_owned()));
        assert_eq!(parse_str("develop a.dng --exposure"), Err("--exposure needs a value".to_owned()));
        assert_eq!(parse_str("develop a.dng --exposure lots"), Err("'lots' is not a number of stops".to_owned()));
        assert_eq!(parse_str("develop a.dng --quality 0"), Err("quality must be 1-100, not '0'".to_owned()));
        assert_eq!(parse_str("develop a.dng b.dng"), Err("unexpected argument 'b.dng'".to_owned()));
        assert_eq!(parse_str("develop a.dng --fast"), Err("unknown option '--fast'".to_owned()));
        assert_eq!(parse_str("frobnicate"), Err("unknown command 'frobnicate'".to_owned()));
        assert_eq!(parse_str("gpu now"), Err("unexpected argument 'now'".to_owned()));
    }

    #[test]
    fn no_arguments_shows_help() {
        assert_eq!(parse_str(""), Ok(Command::Help));
    }
}
