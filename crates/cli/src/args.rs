//! Command-line parsing (std only).

use std::path::PathBuf;

pub const USAGE: &str = "\
Usage:
  aktis-cli develop <file> [-o <out.jpg>] [--exposure <stops>] [--denoise <grain %>]
              [--denoise-color <colour %>] [--quality <1-100>] [--cpu]
              [--profile <name>] [--lens <name>] [--lens-zoom <factor>] [--no-lens] [selection]
  aktis-cli mask <file> <selection> [-o <overlay.jpg>] [--mask-out <mask.jpg>]
  aktis-cli info <file>
  aktis-cli lenses [<words>...]
  aktis-cli profiles <file>
  aktis-cli models [--verify]
  aktis-cli models --download <what>... [--yes]
  aktis-cli gpu
  aktis-cli tag <files...> [--model <dir>] --keywords <file> [--template <text>] [--min <0-1>]
          [--top <n>] [--search <text>] [--quantized]
  aktis-cli help

develop   Develops a RAW file to an sRGB JPEG. Without -o the JPEG goes next to
          the RAW file; an existing file is never overwritten. With a
          selection, the exposure applies only inside it (a layer masked
          to the selection).
          --denoise turns on the AI denoise (SCUNet), before the exposure:
          how much of the grain to remove, 0-100 (80 is the recommended
          value); --denoise-color sets how much of the colour blotches to
          remove, 0-100 (default 100). The model is looked for in
          AKTIS_DENOISE_MODELS, then in a models folder near aktis-cli.
          Lens corrections (lensfun) are applied when camera and lens are known.
          --profile    camera profile by name (see `aktis-cli profiles`); default: the
                       camera's built-in matrices
          --lens       lens by its database name, when the file does not say it
                       or says it wrongly (see `aktis-cli lenses`)
          --lens-zoom  magnification after correction (default: no empty borders)
          --no-lens    no lens corrections
info      Shows camera, lens, shooting data and the corrections that would apply.
lenses    Lists the lenses of the lens database whose name contains every word.
profiles  Lists the camera profiles (DCP) in the editor's profiles folder for a RAW file's camera.
mask      Makes an AI selection and saves a preview of it (the selection in
          red over the photo), and with --mask-out the mask itself.
models    Lists what the AI tools download (ONNX Runtime and the models), where
          it goes and whether it is installed; --verify also checks SHA-256.
          --download installs what is missing: a tool (denoise, subject,
          objects, people, parts, tagging), a pack id from the list, or all.
          The AI commands also offer the download the first time they need it.
gpu       Shows the GPU that would be used.
tag       Suggests keywords for RAW or JPEG files with a local AI model. --model is
          a SigLIP 2 ONNX folder (default: the one `aktis-cli models` installs),
          --keywords a keyword list in Lightroom's text format (one per line,
          tabs for levels, {synonyms}). --template puts each
          keyword in a sentence, e.g. \"una foto di {}\". --search ranks the files by
          how well they match a description instead.

<file> is a RAW/DNG file, or an 8-bit binary PPM (.ppm/.pnm) for testing.

Selection (coordinates are fractions 0-1 of the upright photo, with a
decimal point, x from the left, y from the top). Each part is added to the
selection, unless --subtract or --intersect comes before it:
  --subject            the main subject
  --people             every person, each as its own part of the mask
  --parts <list>       parts of every person: hair, face-skin, body-skin,
                       clothes, accessories, eyebrows, eye-sclera, irises,
                       lips, teeth (comma separated)
  --detect <class>     every object of a COCO class (car, dog, bicycle ...)
  --point <x,y>        the object under this click (repeat to add clicks)
  --not <x,y>          a click on something to leave out of that object
  --box <x0,y0,x1,y1>  the object inside this rectangle
  --brush <r:x,y:x,y>  paint a stroke of radius r (fraction of the long side)
                       through the points
  --erase <r:x,y:...>  erase a stroke (same as --subtract --brush)
  --add, --subtract, --intersect
                       how the parts that follow combine with the ones before
  --edge-aware         brush strokes follow the photo's edges
  --invert             select everything else (the background)
  --feather <percent>  soften the edge (0-10 % of the long side)
  --no-refine          do not snap the edges to the photo

AI models go in the folder shown by `aktis-cli models` (or $AKTIS_MODELS_DIR); the
ONNX Runtime library is found through $ORT_DYLIB_PATH, next to aktis-cli, or in
that folder.";

/// What to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Develop(DevelopArgs),
    Mask(MaskArgs),
    Info(PathBuf),
    Lenses(Vec<String>),
    Profiles(PathBuf),
    Models {
        verify: bool,
    },
    /// Install what the AI tools need: tool names, pack ids or `all`.
    Download {
        what: Vec<String>,
        yes: bool,
    },
    Gpu,
    Tag(TagArgs),
    Help,
}

/// Arguments of `tag`.
#[derive(Debug, Clone, PartialEq)]
pub struct TagArgs {
    pub inputs: Vec<PathBuf>,
    /// SigLIP 2 folder; `None` for the one in the models folder.
    pub model: Option<PathBuf>,
    pub keywords: Option<PathBuf>,
    pub template: String,
    pub min_probability: f32,
    pub top: usize,
    pub search: Option<String>,
    pub quantized: bool,
}

/// How a part of the selection combines with the parts before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Op {
    #[default]
    Add,
    Subtract,
    Intersect,
}

/// Clicks of an object: x, y and whether the click includes.
pub type Clicks = Vec<(f32, f32, bool)>;
/// A rectangle: x0, y0, x1, y1.
pub type Rect = Option<[f32; 4]>;

/// One part of a selection given on the command line (coordinates upright, 0-1).
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Subject,
    /// Every object of these COCO classes (`--people` is class 0).
    Detect(usize),
    /// Parts of every person.
    Parts(Vec<core_model::BodyPart>),
    /// Clicks (x, y, include) and a rectangle (x0, y0, x1, y1).
    Object {
        points: Vec<(f32, f32, bool)>,
        rect: Option<[f32; 4]>,
    },
    /// A brush stroke: radius and path.
    Brush {
        radius: f32,
        path: Vec<(f32, f32)>,
    },
}

/// An AI and brush selection given on the command line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Selection {
    /// Parts in order, each with how it combines with the ones before.
    pub steps: Vec<(Op, Step)>,
    /// The `--add` / `--subtract` / `--intersect` in force.
    pub op: Op,
    pub edge_aware: bool,
    pub invert: bool,
    /// Feather as a percentage of the long side.
    pub feather: f32,
    pub no_refine: bool,
}

impl Selection {
    /// Whether anything is selected.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Adds a click or a rectangle to the object being described (a new one
    /// after any other kind of part or a change of --add/--subtract).
    fn object(&mut self) -> (&mut Clicks, &mut Rect) {
        let op = self.op;
        if !matches!(self.steps.last(), Some((o, Step::Object { .. })) if *o == op) {
            self.steps.push((op, Step::Object { points: Vec::new(), rect: None }));
        }
        match self.steps.last_mut() {
            Some((_, Step::Object { points, rect })) => (points, rect),
            _ => unreachable!("an object step was just pushed"),
        }
    }
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
    /// Camera profile by name; `None` for the built-in matrices.
    pub profile: Option<String>,
    /// Lens chosen by name.
    pub lens: Option<String>,
    pub lens_zoom: Option<f32>,
    pub no_lens: bool,
    pub selection: Selection,
}

/// Arguments of `mask`.
#[derive(Debug, Clone, PartialEq)]
pub struct MaskArgs {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub mask_out: Option<PathBuf>,
    pub selection: Selection,
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
        "models" => match it.next().as_deref() {
            None => Ok(Command::Models { verify: false }),
            Some("--verify") => no_more(it).map(|()| Command::Models { verify: true }),
            Some("--download") => {
                let (mut what, mut yes) = (Vec::new(), false);
                for arg in it {
                    match arg.as_str() {
                        "--yes" | "-y" => yes = true,
                        flag if flag.starts_with('-') => return Err(format!("unknown option '{flag}'")),
                        name => what.push(name.to_owned()),
                    }
                }
                if what.is_empty() {
                    return Err("--download needs what to install: a tool name, a pack id or all".to_owned());
                }
                Ok(Command::Download { what, yes })
            }
            Some(other) => Err(format!("unknown option '{other}'")),
        },
        "develop" => parse_develop(it).map(Command::Develop),
        "lenses" => Ok(Command::Lenses(it.collect())),
        "profiles" => {
            let file = it.next().ok_or("profiles needs a RAW file")?;
            no_more(it)?;
            Ok(Command::Profiles(PathBuf::from(file)))
        }
        "tag" => parse_tag(it).map(Command::Tag),
        "mask" => parse_mask(it).map(Command::Mask),
        other => Err(format!("unknown command '{other}'")),
    }
}

fn no_more(mut it: impl Iterator<Item = String>) -> Result<(), String> {
    it.next().map_or(Ok(()), |extra| Err(format!("unexpected argument '{extra}'")))
}

/// Comma-separated fractions in [0, 1], exactly `n` of them.
fn fractions(v: &str, n: usize, what: &str) -> Result<Vec<f32>, String> {
    let parts: Vec<f32> = v
        .split(',')
        .map(|p| p.trim().parse::<f32>().ok().filter(|x| x.is_finite() && (0.0..=1.0).contains(x)))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("{what} needs {n} numbers between 0 and 1 separated by commas, not '{v}'"))?;
    if parts.len() == n {
        Ok(parts)
    } else {
        Err(format!("{what} needs {n} numbers between 0 and 1 separated by commas, not '{v}'"))
    }
}

/// Consumes one selection option if `arg` is one; returns whether it was.
fn selection_option(
    arg: &str,
    value: &mut dyn FnMut(&str) -> Result<String, String>,
    s: &mut Selection,
) -> Result<bool, String> {
    match arg {
        "--add" => s.op = Op::Add,
        "--subtract" => s.op = Op::Subtract,
        "--intersect" => s.op = Op::Intersect,
        "--edge-aware" => s.edge_aware = true,
        "--subject" => s.steps.push((s.op, Step::Subject)),
        "--people" => s.steps.push((s.op, Step::Detect(ai_segment::PERSON))),
        "--detect" => {
            let v = value("--detect")?;
            let class = ai_segment::class_index(&v).ok_or_else(|| format!("'{v}' is not a known kind of object"))?;
            s.steps.push((s.op, Step::Detect(class)));
        }
        "--parts" => {
            let v = value("--parts")?;
            let parts = v
                .split(',')
                .map(|p| core_model::BodyPart::from_id(p.trim()).ok_or_else(|| format!("'{p}' is not a body part")))
                .collect::<Result<Vec<_>, String>>()?;
            s.steps.push((s.op, Step::Parts(parts)));
        }
        "--point" | "--not" => {
            let v = value(arg)?;
            let p = fractions(&v, 2, arg)?;
            s.object().0.push((p[0], p[1], arg == "--point"));
        }
        "--box" => {
            let v = value("--box")?;
            let r = fractions(&v, 4, "--box")?;
            *s.object().1 = Some([r[0], r[1], r[2], r[3]]);
        }
        "--brush" | "--erase" => {
            let v = value(arg)?;
            let mut pieces = v.split(':');
            let radius = pieces
                .next()
                .and_then(|r| r.trim().parse::<f32>().ok())
                .filter(|r| *r > 0.0 && *r <= core_model::MAX_BRUSH_RADIUS)
                .ok_or_else(|| format!("{arg} needs a radius 0-0.5 first, as in 0.03:0.2,0.4:0.3,0.45"))?;
            let path =
                pieces.map(|p| fractions(p, 2, arg).map(|xy| (xy[0], xy[1]))).collect::<Result<Vec<_>, String>>()?;
            if path.is_empty() {
                return Err(format!("{arg} needs at least one point after the radius"));
            }
            let op = if arg == "--erase" { Op::Subtract } else { s.op };
            s.steps.push((op, Step::Brush { radius, path }));
        }
        "--invert" => s.invert = true,
        "--no-refine" => s.no_refine = true,
        "--feather" => {
            let v = value("--feather")?;
            s.feather = v
                .replace(',', ".")
                .parse::<f32>()
                .ok()
                .filter(|f| (0.0..=10.0).contains(f))
                .ok_or_else(|| format!("feather must be 0-10 (percent), not '{v}'"))?;
        }
        _ => return Ok(false),
    }
    Ok(true)
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
        profile: None,
        lens: None,
        lens_zoom: None,
        no_lens: false,
        selection: Selection::default(),
    };
    let (mut grain, mut color) = (None, None);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        if selection_option(&arg, &mut value, &mut a.selection)? {
            continue;
        }
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
    if grain.is_some() || color.is_some() {
        a.denoise = Some((grain.unwrap_or(DEFAULT_DENOISE_GRAIN), color.unwrap_or(DEFAULT_DENOISE_COLOR)));
    }
    Ok(a)
}

fn parse_tag(mut it: impl Iterator<Item = String>) -> Result<TagArgs, String> {
    let mut a = TagArgs {
        inputs: Vec::new(),
        model: None,
        keywords: None,
        template: "{}".to_owned(),
        min_probability: 0.005,
        top: 10,
        search: None,
        quantized: false,
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--model" => a.model = Some(PathBuf::from(value("--model")?)),
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
    if a.inputs.is_empty() {
        return Err("tag needs at least one image".to_owned());
    }
    if a.keywords.is_none() && a.search.is_none() {
        return Err("tag needs --keywords <file> or --search <text>".to_owned());
    }
    Ok(a)
}

fn parse_mask(mut it: impl Iterator<Item = String>) -> Result<MaskArgs, String> {
    let mut input = None;
    let mut a = MaskArgs { input: PathBuf::new(), output: None, mask_out: None, selection: Selection::default() };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        if selection_option(&arg, &mut value, &mut a.selection)? {
            continue;
        }
        match arg.as_str() {
            "-o" | "--output" => a.output = Some(PathBuf::from(value("-o")?)),
            "--mask-out" => a.mask_out = Some(PathBuf::from(value("--mask-out")?)),
            flag if flag.starts_with('-') && flag.len() > 1 => return Err(format!("unknown option '{flag}'")),
            file if input.is_none() => input = Some(PathBuf::from(file)),
            extra => return Err(format!("unexpected argument '{extra}'")),
        }
    }
    a.input = input.ok_or("mask needs a file")?;
    if a.selection.is_empty() {
        return Err(
            "mask needs a selection: --subject, --people, --parts, --detect, --point, --box or --brush".to_owned()
        );
    }
    Ok(a)
}

/// Recommended amounts of the AI denoise (a medium strength).
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
                profile: None,
                lens: None,
                lens_zoom: None,
                no_lens: false,
                selection: Selection::default(),
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
        assert_eq!((a.output, a.exposure, a.denoise, a.quality, a.cpu_only), (None, None, None, 92, false));
        assert!(a.selection.is_empty());
    }

    #[test]
    fn negative_exposure_is_a_value_not_an_option() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --exposure -1.5") else { panic!("expected develop") };
        assert_eq!(a.exposure, Some(-1.5));
    }

    #[test]
    fn either_denoise_option_turns_it_on_with_the_recommended_default_for_the_other() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --denoise 50") else { panic!("expected develop") };
        assert_eq!(a.denoise, Some((50.0, 100.0)));
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --denoise-color 0") else { panic!("expected develop") };
        assert_eq!(a.denoise, Some((80.0, 0.0)));
    }

    #[test]
    fn develop_takes_a_selection_for_the_exposure() {
        let Ok(Command::Develop(a)) = parse_str("develop a.nef --exposure 1 --subject --invert") else {
            panic!("expected develop")
        };
        assert_eq!(a.selection.steps, vec![(Op::Add, Step::Subject)]);
        assert!(a.selection.invert);
    }

    #[test]
    fn mask_collects_every_kind_of_selection() {
        let c = parse_str(
            "mask a.ppm --people --detect Dog --point 0.5,0.25 --not 0.1,0.2 --box 0.1,0.1,0.9,0.8 \
             --feather 2 --no-refine -o p.jpg --mask-out m.jpg",
        )
        .expect("parses");
        let Command::Mask(m) = c else { panic!("expected mask") };
        assert_eq!(
            m.selection.steps,
            vec![
                (Op::Add, Step::Detect(0)),
                (Op::Add, Step::Detect(16)),
                (
                    Op::Add,
                    Step::Object {
                        points: vec![(0.5, 0.25, true), (0.1, 0.2, false)],
                        rect: Some([0.1, 0.1, 0.9, 0.8])
                    }
                ),
            ]
        );
        assert!((m.selection.feather - 2.0).abs() < 1e-6 && m.selection.no_refine);
        assert_eq!((m.output, m.mask_out), (Some(PathBuf::from("p.jpg")), Some(PathBuf::from("m.jpg"))));
    }

    #[test]
    fn subtract_intersect_parts_and_brushes_build_the_steps_in_order() {
        let c = parse_str(
            "mask a.ppm --people --subtract --parts face-skin,lips --point 0.5,0.5 --add --brush 0.05:0.1,0.2:0.3,0.4 \
             --erase 0.02:0.5,0.5 --intersect --subject --edge-aware",
        )
        .expect("parses");
        let Command::Mask(m) = c else { panic!("expected mask") };
        use core_model::BodyPart::{FaceSkin, Lips};
        assert_eq!(
            m.selection.steps,
            vec![
                (Op::Add, Step::Detect(0)),
                (Op::Subtract, Step::Parts(vec![FaceSkin, Lips])),
                (Op::Subtract, Step::Object { points: vec![(0.5, 0.5, true)], rect: None }),
                (Op::Add, Step::Brush { radius: 0.05, path: vec![(0.1, 0.2), (0.3, 0.4)] }),
                (Op::Subtract, Step::Brush { radius: 0.02, path: vec![(0.5, 0.5)] }),
                (Op::Intersect, Step::Subject),
            ]
        );
        assert!(m.selection.edge_aware);
    }

    #[test]
    fn mask_mistakes_are_explained() {
        assert_eq!(
            parse_str("mask a.nef"),
            Err("mask needs a selection: --subject, --people, --parts, --detect, --point, --box or --brush".to_owned())
        );
        assert_eq!(parse_str("mask a.nef --parts hair,nose"), Err("'nose' is not a body part".to_owned()));
        assert!(parse_str("mask a.nef --brush 0.05").is_err());
        assert!(parse_str("mask a.nef --brush 2:0.1,0.1").is_err());
        assert_eq!(parse_str("mask a.nef --detect unicorn"), Err("'unicorn' is not a known kind of object".to_owned()));
        let bad_point = "--point needs 2 numbers between 0 and 1 separated by commas, not '0.5'";
        assert_eq!(parse_str("mask a.nef --point 0.5"), Err(bad_point.to_owned()));
        assert!(parse_str("mask a.nef --point 1.5,0.2").is_err());
        assert!(parse_str("mask a.nef --box 0,0,1").is_err());
        assert!(parse_str("mask a.nef --subject --feather 20").is_err());
    }

    #[test]
    fn models_lists_or_verifies() {
        assert_eq!(parse_str("models"), Ok(Command::Models { verify: false }));
        assert_eq!(parse_str("models --verify"), Ok(Command::Models { verify: true }));
        assert_eq!(parse_str("models --fast"), Err("unknown option '--fast'".to_owned()));
        assert_eq!(
            parse_str("models --download denoise tagging --yes"),
            Ok(Command::Download { what: vec!["denoise".to_owned(), "tagging".to_owned()], yes: true })
        );
        assert!(parse_str("models --download").is_err());
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
    fn tag_with_options_and_several_files() {
        let Ok(Command::Tag(a)) =
            parse_str("tag a.nef b.jpg --model m --keywords k.txt --template x{}y --min 0,2 --top 3 --quantized")
        else {
            panic!("expected tag")
        };
        assert_eq!(a.inputs, vec![PathBuf::from("a.nef"), PathBuf::from("b.jpg")]);
        assert_eq!((a.model, a.keywords), (Some(PathBuf::from("m")), Some(PathBuf::from("k.txt"))));
        assert_eq!((a.template.as_str(), a.top, a.quantized), ("x{}y", 3, true));
        assert!((a.min_probability - 0.2).abs() < 1e-6);
    }

    #[test]
    fn tag_mistakes_are_explained() {
        let Ok(Command::Tag(a)) = parse_str("tag a.jpg --keywords k") else { panic!("expected tag") };
        assert_eq!(a.model, None, "without --model the installed SigLIP 2 is used");
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
