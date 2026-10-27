//! Command-line parsing (std only).

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage:
  rne develop <file> [-o <out.jpg>] [--exposure <stops>] [--quality <1-100>] [--cpu]
  rne info <file>
  rne gpu
  rne tag <files...> --model <dir> --keywords <file> [--template <text>] [--min <0-1>]
          [--top <n>] [--search <text>] [--quantized]
  rne help

develop   Develops a RAW file to an sRGB JPEG. Without -o the JPEG goes next to
          the RAW file; an existing file is never overwritten.
info      Shows camera, size and sensor layout of a RAW file.
gpu       Shows the GPU that would be used.
tag       Suggests keywords for RAW or JPEG files with a local AI model. --model is
          a SigLIP 2 ONNX folder, --keywords a keyword list in Lightroom's text
          format (one per line, tabs for levels, {synonyms}). --template puts each
          keyword in a sentence, e.g. \"una foto di {}\". --search ranks the files by
          how well they match a description instead.";

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Develop(DevelopArgs),
    Info(PathBuf),
    Gpu,
    Tag(TagArgs),
    Help,
}

/// Arguments of `tag`.
#[derive(Debug, Clone, PartialEq)]
pub struct TagArgs {
    pub inputs: Vec<PathBuf>,
    pub model: PathBuf,
    pub keywords: Option<PathBuf>,
    pub template: String,
    pub min_probability: f32,
    pub top: usize,
    pub search: Option<String>,
    pub quantized: bool,
}

/// Arguments of `develop`.
#[derive(Debug, Clone, PartialEq)]
pub struct DevelopArgs {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub exposure: Option<f32>,
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
        "tag" => parse_tag(it).map(Command::Tag),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn no_more(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    it.next().map_or(Ok(()), |extra| Err(format!("unexpected argument '{extra}'")))
}

fn parse_develop(mut it: impl Iterator<Item = String>) -> Result<DevelopArgs, String> {
    let mut input = None;
    let mut a =
        DevelopArgs { input: PathBuf::new(), output: None, exposure: None, quality: DEFAULT_QUALITY, cpu_only: false };
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
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            file if input.is_none() => input = Some(PathBuf::from(file)),
            extra => return Err(format!("unexpected argument '{extra}'")),
        }
    }
    a.input = input.ok_or("develop needs a RAW file")?;
    Ok(a)
}

fn parse_tag(mut it: impl Iterator<Item = String>) -> Result<TagArgs, String> {
    let mut a = TagArgs {
        inputs: Vec::new(),
        model: PathBuf::new(),
        keywords: None,
        template: "{}".to_owned(),
        min_probability: 0.05,
        top: 10,
        search: None,
        quantized: false,
    };
    let mut model = None;
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(value("--model")?)),
            "--keywords" => a.keywords = Some(PathBuf::from(value("--keywords")?)),
            "--template" => {
                a.template = value("--template")?;
                if !a.template.contains("{}") {
                    return Err("--template must contain {} where the keyword goes".to_owned());
                }
            }
            "--min" => {
                let v = value("--min")?;
                a.min_probability = v
                    .replace(',', ".")
                    .parse()
                    .ok()
                    .filter(|p: &f32| (0.0..=1.0).contains(p))
                    .ok_or_else(|| format!("--min must be between 0 and 1, not '{v}'"))?;
            }
            "--top" => {
                let v = value("--top")?;
                a.top = v
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("--top must be a positive number, not '{v}'"))?;
            }
            "--search" => a.search = Some(value("--search")?),
            "--quantized" => a.quantized = true,
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            file => a.inputs.push(PathBuf::from(file)),
        }
    }
    a.model = model.ok_or("tag needs --model <folder>")?;
    if a.inputs.is_empty() {
        return Err("tag needs at least one image".to_owned());
    }
    if a.keywords.is_none() && a.search.is_none() {
        return Err("tag needs --keywords <file> or --search <text>".to_owned());
    }
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
            })
        );
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
    fn tag_with_options_and_several_files() {
        let Ok(Command::Tag(a)) =
            parse_str("tag a.nef b.jpg --model m --keywords k.txt --template x{}y --min 0,2 --top 3 --quantized")
        else {
            panic!("expected tag")
        };
        assert_eq!(a.inputs, vec![PathBuf::from("a.nef"), PathBuf::from("b.jpg")]);
        assert_eq!((a.model, a.keywords), (PathBuf::from("m"), Some(PathBuf::from("k.txt"))));
        assert_eq!((a.template.as_str(), a.top, a.quantized), ("x{}y", 3, true));
        assert!((a.min_probability - 0.2).abs() < 1e-6);
    }

    #[test]
    fn tag_mistakes_are_explained() {
        assert_eq!(parse_str("tag a.jpg --keywords k"), Err("tag needs --model <folder>".to_owned()));
        assert_eq!(parse_str("tag --model m --keywords k"), Err("tag needs at least one image".to_owned()));
        assert_eq!(parse_str("tag a.jpg --model m"), Err("tag needs --keywords <file> or --search <text>".to_owned()));
        assert_eq!(
            parse_str("tag a.jpg --model m --keywords k --template plain"),
            Err("--template must contain {} where the keyword goes".to_owned())
        );
        assert_eq!(
            parse_str("tag a.jpg --model m --keywords k --min 2"),
            Err("--min must be between 0 and 1, not '2'".to_owned())
        );
    }

    #[test]
    fn no_arguments_shows_help() {
        assert_eq!(parse_str(""), Ok(Command::Help));
    }
}
