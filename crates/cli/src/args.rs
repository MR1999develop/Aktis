//! Command-line parsing (std only).

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage:
  rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--denoise <grain %>]
              [--denoise-color <colour %>] [--quality <1-100>] [--cpu]
  rne info <file>
  rne gpu
  rne help

develop   Develops a RAW file to an sRGB JPEG. Without -o the JPEG goes next to
          the RAW file; an existing file is never overwritten.
          --denoise adds the AI denoise node (SCUNet) after the exposure:
          how much of the grain to remove, 0-100 (80 is the recommended
          value); --denoise-color sets how much of the colour blotches to
          remove, 0-100 (default 100). The model is looked for in
          RNE_DENOISE_MODELS, then in a models folder near rne.
info      Shows camera, size and sensor layout of a RAW file.
gpu       Shows the GPU that would be used.";

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Develop(DevelopArgs),
    Info(PathBuf),
    Gpu,
    Help,
}

/// Arguments of `develop`.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopArgs {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub exposure: Option<f32>,
    /// AI denoise: grain and colour-blotch amounts in percent.
    pub denoise: Option<(f32, f32)>,
    pub quality: u8,
    pub cpu_only: bool,
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
        denoise: None,
        quality: DEFAULT_QUALITY,
        cpu_only: false,
    };
    let (mut grain, mut color) = (None, None);
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
            "--denoise" => grain = Some(percent("--denoise", &value("--denoise")?)?),
            "--denoise-color" => color = Some(percent("--denoise-color", &value("--denoise-color")?)?),
            "--quality" => {
                let v = value("--quality")?;
                a.quality = v
                    .parse()
                    .ok()
                    .filter(|q| (1..=100).contains(q))
                    .ok_or_else(|| format!("quality must be 1-100, not '{v}'"))?;
            }
            "--cpu" => a.cpu_only = true,
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            file if input.is_none() => input = Some(PathBuf::from(file)),
            extra => return Err(format!("unexpected argument '{extra}'")),
        }
    }
    a.input = input.ok_or("develop needs a RAW file")?;
    if grain.is_some() || color.is_some() {
        a.denoise = Some((grain.unwrap_or(DEFAULT_DENOISE_GRAIN), color.unwrap_or(DEFAULT_DENOISE_COLOR)));
    }
    Ok(a)
}

/// Recommended amounts of the AI denoise node (an earlier denoiser's "Medium").
const DEFAULT_DENOISE_GRAIN: f32 = 80.0;
const DEFAULT_DENOISE_COLOR: f32 = 100.0;

/// A percentage 0-100, decimal point or comma.
fn percent(name: &str, v: &str) -> Result<f32, String> {
    v.replace(',', ".")
        .parse::<f32>()
        .ok()
        .filter(|p| (0.0..=100.0).contains(p))
        .ok_or_else(|| format!("{name} must be a percentage 0-100, not '{v}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Command, String> {
        parse(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn develop_with_every_option() {
        let c = parse_str("develop a.dng -o b.jpg --exposure 0,5 --denoise 60 --denoise-color 40,5 --quality 80 --cpu")
            .expect("parses");
        assert_eq!(
            c,
            Command::Develop(DevelopArgs {
                input: PathBuf::from("a.dng"),
                output: Some(PathBuf::from("b.jpg")),
                exposure: Some(0.5),
                denoise: Some((60.0, 40.5)),
                quality: 80,
                cpu_only: true,
            })
        );
    }

    #[test]
    fn develop_defaults() {
        let Ok(Command::Develop(a)) = parse_str("develop a.arw") else { panic!("expected develop") };
        assert_eq!((a.output, a.exposure, a.denoise, a.quality, a.cpu_only), (None, None, None, 92, false));
    }

    #[test]
    fn negative_exposure_is_a_value_not_an_option() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --exposure -1.5") else { panic!("expected develop") };
        assert_eq!(a.exposure, Some(-1.5));
    }

    #[test]
    fn either_denoise_option_adds_the_node_with_the_recommended_default_for_the_other() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --denoise 50") else { panic!("expected develop") };
        assert_eq!(a.denoise, Some((50.0, 100.0)));
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --denoise-color 0") else { panic!("expected develop") };
        assert_eq!(a.denoise, Some((80.0, 0.0)));
    }

    #[test]
    fn mistakes_are_explained() {
        assert_eq!(
            parse_str("develop a.dng --denoise 150"),
            Err("--denoise must be a percentage 0-100, not '150'".to_owned())
        );
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
