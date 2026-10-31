use gliff_windows::palette::Palette;

use gliff_windows::strip::Measure;

use super::Approx;
use super::Options;
use super::file_name;
use super::judge;
use super::scene;
use super::scene::SCALES;
use super::selection;
use super::shapes;

const PAL: Palette = Palette::DARK;

fn problems_of(scene: &scene::Scene, scale: u32) -> Vec<String> {
    let built = scene::build(scene, scale, &Approx);
    let (px, w, h) = shapes(&built, &PAL);
    let ops = built.ops(&PAL);
    judge::problems(
        scene,
        &built,
        &ops,
        &judge::Picture {
            px: &px,
            width: w,
            height: h,
            text: false,
        },
        &PAL,
    )
}

#[test]
fn every_scene_is_right_at_every_scale() {
    let mut all = Vec::new();
    for scene in scene::scenes() {
        for scale in SCALES {
            for p in problems_of(&scene, scale) {
                all.push(format!("{} at {scale}%: {p}", scene.name));
            }
        }
    }
    assert!(all.is_empty(), "{}", all.join("\n"));
}

#[test]
fn the_strip_is_the_design_height_at_each_scale() {
    let scene = &scene::scenes()[0];
    let heights: Vec<i32> = SCALES
        .iter()
        .map(|s| scene::build(scene, *s, &Approx).metrics.strip_height)
        .collect();
    assert_eq!(heights, [32, 40, 48, 64]);
}

#[test]
fn the_scenes_cover_what_the_design_lists() {
    let names: Vec<&str> = scene::scenes().iter().map(|s| s.name).collect();
    for wanted in [
        "one-tab",
        "three-tabs-connecting",
        "ten-tabs",
        "dots",
        "failed",
        "hover-tab",
        "hover-dot",
        "hover-add",
        "narrow-chevron",
        "narrow-hover-chevron",
        "dragging",
        "long-name",
        "short-names",
        "long-host",
        "near-identical-ips",
        "narrow-near-identical-ips",
        "near-identical-hosts",
        "bar-empty",
        "bar-typed",
        "bar-unfocused",
        "bar-error",
        "bar-hover-go",
        "bar-hover-list",
        "narrow-bar",
        "bar-no-tabs",
    ] {
        assert!(names.contains(&wanted), "no scene {wanted}");
    }
}

#[test]
fn only_the_narrow_scenes_have_a_chevron() {
    for scene in scene::scenes() {
        for scale in SCALES {
            let built = scene::build(&scene, scale, &Approx);
            assert_eq!(
                built.layout.chevron.is_some(),
                scene.name.starts_with("narrow"),
                "{} at {scale}%",
                scene.name
            );
        }
    }
}

#[test]
fn the_judge_sees_a_dot_that_is_too_small() {
    let scene = &scene::scenes()[0];
    let mut built = scene::build(scene, 100, &Approx);
    built.metrics.dot = 6;
    let (px, w, h) = shapes(&built, &PAL);
    let ops = built.ops(&PAL);
    let found = judge::problems(
        scene,
        &built,
        &ops,
        &judge::Picture {
            px: &px,
            width: w,
            height: h,
            text: false,
        },
        &PAL,
    );
    assert!(
        found.iter().any(|p| p.contains("the dot is 6 px")),
        "{found:?}"
    );
}

#[test]
fn the_judge_sees_overlapping_buttons_and_a_missing_failed_dot() {
    let scene = scene::scenes()
        .into_iter()
        .find(|s| s.name == "failed")
        .expect("scene");
    let mut built = scene::build(&scene, 100, &Approx);
    built.layout.add = built.layout.fullscreen;
    let ops = built.ops(&PAL);
    let blank = vec![0u32; (built.width * built.metrics.strip_height) as usize];
    let found = judge::problems(
        &scene,
        &built,
        &ops,
        &judge::Picture {
            px: &blank,
            width: built.width,
            height: built.metrics.strip_height,
            text: false,
        },
        &PAL,
    );
    assert!(
        found.iter().any(|p| p == "add overlaps fullscreen"),
        "{found:?}"
    );
    assert!(
        found.iter().any(|p| p.contains("Failed dot does not show")),
        "{found:?}"
    );
}

#[test]
fn the_judge_sees_a_name_that_is_not_drawn() {
    let scene = &scene::scenes()[0];
    let built = scene::build(scene, 100, &Approx);
    let (px, w, h) = shapes(&built, &PAL);
    let ops = built.ops(&PAL);
    let found = judge::problems(
        scene,
        &built,
        &ops,
        &judge::Picture {
            px: &px,
            width: w,
            height: h,
            text: true,
        },
        &PAL,
    );
    assert!(
        found.iter().any(|p| p.contains("no name is drawn")),
        "{found:?}"
    );
}

#[test]
fn pictures_are_named_with_their_scale() {
    let scene = scene::scenes()
        .into_iter()
        .find(|s| s.name == "failed")
        .expect("scene");
    assert_eq!(file_name(&scene, 200, false), "strip-failed-200pct.png");
    assert_eq!(
        file_name(&scene, 125, true),
        "strip-failed-shapes-125pct.png"
    );
}

#[test]
fn an_unknown_scene_or_scale_is_refused_and_a_choice_is_kept() {
    let ask = |scenes: &[&str], scales: &[u32]| Options {
        out: std::env::temp_dir(),
        scales: scales.to_vec(),
        scenes: scenes.iter().map(|s| (*s).to_owned()).collect(),
        shapes_only: true,
    };
    assert!(selection(&ask(&["nope"], &[])).is_err());
    assert!(selection(&ask(&[], &[110])).is_err());
    let (scenes, scales) = selection(&ask(&["failed"], &[150])).expect("known");
    assert_eq!((scenes.len(), scales), (1, vec![150]));
    let (all, scales) = selection(&ask(&[], &[])).expect("all");
    assert_eq!((all.len(), scales), (26, vec![100, 125, 150, 200]));
}

#[test]
fn a_run_writes_one_png_per_scene_and_scale() {
    let dir = std::env::temp_dir().join(format!("gwv-strip-{}", std::process::id()));
    let options = Options {
        out: dir.clone(),
        scales: vec![100, 200],
        scenes: vec!["failed".to_owned(), "narrow-chevron".to_owned()],
        shapes_only: true,
    };
    let line = super::run(&options);
    assert!(line.ok, "{}", line.to_json());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "strip-failed-shapes-100pct.png",
            "strip-failed-shapes-200pct.png",
            "strip-narrow-chevron-shapes-100pct.png",
            "strip-narrow-chevron-shapes-200pct.png",
        ]
    );
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(dir.join("strip-failed-shapes-200pct.png")).expect("open"),
    ));
    let info = decoder.read_info().expect("png").info().clone();
    assert_eq!((info.width, info.height), (1280, 64));
    std::fs::remove_dir_all(&dir).expect("clean up");
}

fn named(name: &str) -> scene::Scene {
    scene::scenes()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scene {name}"))
}

#[test]
fn near_identical_addresses_are_each_drawn_whole_at_every_scale() {
    for name in [
        "long-name",
        "short-names",
        "long-host",
        "near-identical-ips",
        "narrow-near-identical-ips",
        "near-identical-hosts",
    ] {
        let scene = named(name);
        for scale in SCALES {
            let built = scene::build(&scene, scale, &Approx);
            let drawn: Vec<(String, i32, gliff_windows::strip::Rect)> = built
                .ops(&PAL)
                .into_iter()
                .filter_map(|op| match op {
                    gliff_windows::strip::draw::Op::Text {
                        text, size, rect, ..
                    } => Some((text, size, rect)),
                    _ => None,
                })
                .collect();
            let mut seen: Vec<&str> = Vec::new();
            for (text, size, rect) in &drawn {
                assert_eq!(*size, built.layout.text, "{name} at {scale}%");
                assert!(
                    !text.contains('\u{2026}') && !text.contains("..."),
                    "{text}"
                );
                assert!(
                    rect.width() >= Approx.text_width(text, *size),
                    "{name} at {scale}%: {text} is cut"
                );
                assert!(!seen.contains(&text.as_str()), "{text} twice");
                seen.push(text);
            }
            if !built.layout.chevron.is_some() {
                assert_eq!(
                    drawn.len(),
                    scene.tabs.len(),
                    "{name} at {scale}%: all drawn"
                );
                for (n, (full, _)) in scene.tabs.iter().enumerate() {
                    assert_eq!(&drawn[n].0, full, "{name} at {scale}%: tab {}", n + 1);
                }
            }
            assert!((built.metrics.text_floor..=built.metrics.text).contains(&built.layout.text));
        }
    }
}

#[test]
fn the_near_identical_scenes_shrink_the_font_before_they_scroll() {
    let wide = scene::build(&named("near-identical-ips"), 100, &Approx);
    assert_eq!(wide.layout.chevron, None);
    assert!(
        wide.layout.text < wide.metrics.text,
        "a smaller font, whole"
    );
    let narrow = scene::build(&named("narrow-near-identical-ips"), 100, &Approx);
    assert!(narrow.layout.chevron.is_some());
    assert_eq!(narrow.layout.text, narrow.metrics.text_floor);
    let long = scene::build(&named("long-host"), 100, &Approx);
    assert!(
        long.layout.tabs[1].rect.width() > 240,
        "wider than the old cap"
    );
}

#[test]
fn the_judge_sees_a_name_that_does_not_fit_its_place() {
    let scene = named("near-identical-ips");
    let mut built = scene::build(&scene, 100, &Approx);
    built.layout.tabs[11].label.right -= 20;
    let (px, w, h) = shapes(&built, &PAL);
    let ops = built.ops(&PAL);
    let found = judge::problems(
        &scene,
        &built,
        &ops,
        &judge::Picture {
            px: &px,
            width: w,
            height: h,
            text: false,
        },
        &PAL,
    );
    assert!(
        found
            .iter()
            .any(|p| p.contains("tab 12") && p.contains("is cut")),
        "{found:?}"
    );
}
