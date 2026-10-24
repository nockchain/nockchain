// use env_logger::fmt::Color as LogColor;
// use std::collections::hash_map::DefaultHasher;
// use std::hash::{Hash, Hasher};
use std::io::Write;

use termcolor::{ColorSpec, StandardStream, WriteColor};

// pub fn log_format(
//     record: &log::Record,
//     buf: &mut env_logger::fmt::Formatter,
// ) -> Result<(), std::io::Error> {
//     use std::io::Write;
//     let target = record.target();
//     let target = target;
//     let module = target.split("::").nth(1).unwrap_or(target);

//     let (level_color, level_char) = crate::colors::level_colors(record.level());
//     let mut module_style = buf.style();
//     let module_color = module_color(module);
//     module_style
//         .set_color(module_color)
//         .set_dimmed(true)
//         .set_bold(true);

//     let mut level_style = buf.style();
//     level_style.set_color(level_color).set_intense(true);
//     writeln!(
//         buf,
//         "[{}{}] {}",
//         level_style.value(level_char),
//         module_style.value(module),
//         record.args()
//     )
// }

// pub fn level_colors(level: log::Level) -> (LogColor, char) {
//     match level {
//         log::Level::Error => (LogColor::Red, '!'),
//         log::Level::Warn => (LogColor::Yellow, '?'),
//         log::Level::Info => (LogColor::Green, '^'),
//         log::Level::Debug => (LogColor::Blue, '>'),
//         log::Level::Trace => (LogColor::White, '<'),
//     }
// }

// pub fn module_color(module: &str) -> LogColor {
//     let mut hasher = DefaultHasher::new();
//     module.hash(&mut hasher);
//     let hash = hasher.finish();
//     match hash % 6 {
//         0 => LogColor::Rgb(128, 0, 0),     // Dark Red
//         1 => LogColor::Rgb(255, 0, 0),     // Bright Red
//         2 => LogColor::Rgb(255, 69, 0),    // Red-Orange
//         3 => LogColor::Rgb(255, 165, 0),   // Orange
//         4 => LogColor::Rgb(255, 215, 0),   // Gold
//         5 => LogColor::Rgb(255, 255, 255), // White
//         _ => unreachable!(),
//     }
// }

pub(crate) fn print_banner(stdout: &mut StandardStream, banner: &str) {
    let colors = [
        (128, 0, 0),  // Dark Red
        (255, 0, 0),  // Bright Red
        (255, 69, 0), // Red-Orange
    ];

    let lines: Vec<&str> = banner.lines().collect();
    let color_step = (colors.len() - 1) as f32 / (lines.len() - 1) as f32;

    for (i, line) in lines.iter().enumerate() {
        let color_index = (i as f32 * color_step) as usize;
        let (r1, g1, b1) = colors[color_index];
        let (r2, g2, b2) = colors[(color_index + 1).min(colors.len() - 1)];

        for (j, c) in line.chars().enumerate() {
            let t = j as f32 / line.len() as f32;
            let r = (r1 as f32 * (1.0 - t) + r2 as f32 * t) as u8;
            let g = (g1 as f32 * (1.0 - t) + g2 as f32 * t) as u8;
            let b = (b1 as f32 * (1.0 - t) + b2 as f32 * t) as u8;

            let mut color_spec = ColorSpec::new();
            color_spec
                .set_fg(Some(termcolor::Color::Rgb(r, g, b)))
                .set_bold(true);
            stdout.set_color(&color_spec).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
            write!(stdout, "{}", c).unwrap_or_else(|err| {
                panic!(
                    "Panicked with {err:?} at {}:{} (git sha: {:?})",
                    file!(),
                    line!(),
                    option_env!("GIT_SHA")
                )
            });
        }
        writeln!(stdout).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
    }
    stdout.reset().unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
}

#[allow(dead_code)]
pub(crate) fn print_version_info(stdout: &mut StandardStream, info: &[(&str, &str)]) {
    let mut color_spec = ColorSpec::new();
    color_spec
        .set_fg(Some(termcolor::Color::Rgb(255, 69, 0)))
        .set_bold(true);
    stdout.set_color(&color_spec).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    writeln!(stdout, "Nockchain Version Info:").unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    stdout.reset().unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });

    for (i, (label, value)) in info.iter().enumerate() {
        let t = i as f32 / (info.len() - 1) as f32;
        let r = (128.0 * (1.0 - t) + 255.0 * t) as u8;
        let g = (0.0 * (1.0 - t) + 69.0 * t) as u8;
        let b = 0;

        let mut label_spec = ColorSpec::new();
        label_spec
            .set_fg(Some(termcolor::Color::Rgb(r, g, b)))
            .set_bold(true);
        stdout.set_color(&label_spec).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        write!(stdout, "{}: ", label).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

        let mut value_spec = ColorSpec::new();
        value_spec
            .set_fg(Some(termcolor::Color::Rgb(255, 255, 255)))
            .set_bold(false);
        stdout.set_color(&value_spec).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
        writeln!(stdout, "{}", value).unwrap_or_else(|err| {
            panic!(
                "Panicked with {err:?} at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });
    }

    let separator = "════════════════════════════════════════════════════════";
    let mut separator_spec = ColorSpec::new();
    separator_spec
        .set_fg(Some(termcolor::Color::Rgb(255, 69, 0)))
        .set_bold(true);
    stdout.set_color(&separator_spec).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
    writeln!(stdout, "{}", separator).unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });

    stdout.reset().unwrap_or_else(|err| {
        panic!(
            "Panicked with {err:?} at {}:{} (git sha: {:?})",
            file!(),
            line!(),
            option_env!("GIT_SHA")
        )
    });
}
