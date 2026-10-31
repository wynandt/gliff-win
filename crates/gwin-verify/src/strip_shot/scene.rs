//! The strip states `strip-shot` draws, and the strip built for one of them
//! at one scale. Pure: the same code builds the pictures on Windows and the
//! checks on any machine.

use gliff_windows::palette::Palette;
use gliff_windows::strip::DotState;
use gliff_windows::strip::Hit;
use gliff_windows::strip::Layout;
use gliff_windows::strip::Measure;
use gliff_windows::strip::Metrics;
use gliff_windows::strip::TabItem;
use gliff_windows::strip::draw::BarPaint;
use gliff_windows::strip::draw::Drag;
use gliff_windows::strip::draw::Op;
use gliff_windows::strip::draw::Paint;
use gliff_windows::strip::draw::TabPaint;
use gliff_windows::strip::draw::draw;
use gliff_windows::strip::layout;
use gliff_windows::tabs::TabId;
use gliff_windows::tabs::Tabs;

/// The display scales the design names, in percent.
pub const SCALES: [u32; 4] = [100, 125, 150, 200];

/// What the pointer is over, by tab number (0 is the first).
#[derive(Clone, Copy, Debug)]
pub enum Hover {
    Nothing,
    Tab(usize),
    Slot(usize),
    Add,
    Chevron,
    Fullscreen,
    Go,
    List,
}

/// The address bar of a scene.
#[derive(Clone, Copy, Debug)]
pub struct BarScene {
    pub text: &'static str,
    pub focused: bool,
    pub error: bool,
}

/// One state of the strip.
pub struct Scene {
    pub name: &'static str,
    /// Each tab's name and dot.
    pub tabs: Vec<(String, DotState)>,
    /// The tab shown, by number.
    pub shown: Option<usize>,
    /// The strip's width in logical pixels (96 per inch).
    pub width: i32,
    pub hover: Hover,
    /// A tab being dragged, and the tab it would go before.
    pub drag: Option<(usize, Option<usize>)>,
    /// The step of the connecting arc.
    pub phase: u32,
    /// Whether the tabs do not fit, so the chevron shows.
    pub chevron: bool,
    /// The address bar, open in place of the add button.
    pub bar: Option<BarScene>,
}

fn hosts(count: usize) -> Vec<String> {
    (1..=count).map(|i| format!("user@host-{i}")).collect()
}

fn scene(name: &'static str, width: i32, dots: &[DotState]) -> Scene {
    Scene {
        name,
        tabs: hosts(dots.len())
            .into_iter()
            .zip(dots.iter().copied())
            .collect(),
        shown: Some(0),
        width,
        hover: Hover::Nothing,
        drag: None,
        phase: 3,
        chevron: false,
        bar: None,
    }
}

/// Every scene, in the order they are drawn.
pub fn scenes() -> Vec<Scene> {
    use DotState::Connecting;
    use DotState::Failed;
    use DotState::Live;
    use DotState::Reconnecting;
    use DotState::Stopped;
    let ten = [
        Live,
        Live,
        Connecting,
        Live,
        Stopped,
        Reconnecting,
        Live,
        Failed,
        Live,
        Stopped,
    ];
    let three = [Live, Live, Stopped];
    let mut long = scene("long-name", 640, &[Live, Live]);
    long.tabs[1].0 = "a-very-long-user-name@a-very-long-host-name.example.net".to_owned();
    // A 40-character host: the name is longer than any tab used to be.
    let mut long_host = scene("long-host", 800, &[Live, Live, Live]);
    long_host.tabs[1].0 = "ops@build-server-east-region-001.example.net".to_owned();
    // Machines whose addresses differ only at the end.
    let ips = |name: &'static str, width: i32, count: usize| {
        let mut s = scene(name, width, &vec![Live; count]);
        for (i, tab) in s.tabs.iter_mut().enumerate() {
            tab.0 = format!("user@203.0.113.{}", i + 1);
        }
        s.shown = Some(count - 1);
        s
    };
    let mut hosts = scene("near-identical-hosts", 1280, &[Live; 4]);
    for (i, tab) in hosts.tabs.iter_mut().enumerate() {
        tab.0 = format!("deploy@build-server-eng-west.example.net-{}", i + 1);
    }
    // Short names: a tab is its text and padding, with no minimum width.
    let mut short = scene("short-names", 640, &[Live, Live, Connecting, Failed]);
    for (tab, name) in short.tabs.iter_mut().zip(["a@x", "b@y", "me@h1", "ci@z9"]) {
        tab.0 = name.to_owned();
    }
    vec![
        scene("one-tab", 640, &[Live]),
        short,
        scene(
            "three-tabs-connecting",
            640,
            &[Live, Connecting, Reconnecting],
        ),
        Scene {
            shown: Some(3),
            ..scene("ten-tabs", 1280, &ten)
        },
        Scene {
            shown: Some(2),
            ..scene(
                "dots",
                640,
                &[Stopped, Connecting, Live, Reconnecting, Failed],
            )
        },
        Scene {
            shown: Some(1),
            ..scene("failed", 640, &[Live, Failed, Stopped])
        },
        Scene {
            hover: Hover::Tab(1),
            ..scene("hover-tab", 640, &three)
        },
        Scene {
            hover: Hover::Slot(1),
            ..scene("hover-dot", 640, &three)
        },
        Scene {
            hover: Hover::Add,
            ..scene("hover-add", 640, &three)
        },
        Scene {
            hover: Hover::Fullscreen,
            ..scene("hover-fullscreen", 640, &three)
        },
        Scene {
            drag: Some((0, Some(2))),
            ..scene("dragging", 640, &three)
        },
        long,
        long_host,
        ips("near-identical-ips", 1920, 12),
        Scene {
            chevron: true,
            shown: Some(3),
            ..ips("narrow-near-identical-ips", 1280, 12)
        },
        hosts,
        Scene {
            chevron: true,
            shown: Some(3),
            ..scene("narrow-chevron", 480, &ten)
        },
        Scene {
            chevron: true,
            shown: Some(3),
            hover: Hover::Chevron,
            ..scene("narrow-hover-chevron", 480, &ten)
        },
        Scene {
            bar: Some(BarScene {
                text: "",
                focused: true,
                error: false,
            }),
            ..scene("bar-empty", 640, &three)
        },
        Scene {
            bar: Some(BarScene {
                text: "me@build-host.example.net",
                focused: true,
                error: false,
            }),
            ..scene("bar-typed", 640, &three)
        },
        Scene {
            bar: Some(BarScene {
                text: "me@build-host.example.net",
                focused: false,
                error: false,
            }),
            ..scene("bar-unfocused", 640, &three)
        },
        Scene {
            bar: Some(BarScene {
                text: "me@",
                focused: true,
                error: true,
            }),
            ..scene("bar-error", 640, &three)
        },
        Scene {
            hover: Hover::Go,
            bar: Some(BarScene {
                text: "me@build-host.example.net",
                focused: true,
                error: false,
            }),
            ..scene("bar-hover-go", 640, &three)
        },
        Scene {
            hover: Hover::List,
            bar: Some(BarScene {
                text: "",
                focused: true,
                error: false,
            }),
            ..scene("bar-hover-list", 640, &three)
        },
        Scene {
            chevron: true,
            shown: Some(3),
            bar: Some(BarScene {
                text: "me@build-host",
                focused: true,
                error: false,
            }),
            ..scene("narrow-bar", 480, &ten)
        },
        Scene {
            shown: None,
            bar: Some(BarScene {
                text: "",
                focused: true,
                error: false,
            }),
            ..scene("bar-no-tabs", 640, &[])
        },
    ]
}

/// A scene's strip at one scale.
pub struct Built {
    pub metrics: Metrics,
    /// The strip's width in device pixels.
    pub width: i32,
    pub layout: Layout,
    /// Each name's width in the font the layout chose.
    pub text_widths: Vec<i32>,
    pub paints: Vec<TabPaint>,
    pub shown: Option<TabId>,
    pub hover: Hit,
    pub drag: Option<Drag>,
    pub bar: Option<BarPaint<'static>>,
    pub phase: u32,
}

/// The dpi of a scale in percent.
pub fn dpi_of(scale: u32) -> u32 {
    (scale * 96 + 50) / 100
}

pub fn build(scene: &Scene, scale: u32, measure: &dyn Measure) -> Built {
    let metrics = Metrics::at(dpi_of(scale));
    let width = scene.width * metrics.dpi as i32 / 96;
    let mut tabs = Tabs::new();
    for (name, _) in &scene.tabs {
        tabs.open(&name.parse().expect("a scene's tab name is a host"));
    }
    let ids: Vec<TabId> = tabs.iter().map(|t| t.id()).collect();
    let items: Vec<TabItem> = ids
        .iter()
        .zip(&scene.tabs)
        .map(|(id, (name, _))| TabItem {
            id: *id,
            label: name.clone(),
        })
        .collect();
    let paints = ids
        .iter()
        .zip(&scene.tabs)
        .map(|(id, (name, dot))| TabPaint {
            id: *id,
            label: name.clone(),
            dot: *dot,
        })
        .collect();
    let layout = layout(&metrics, width, &items, 0, scene.bar.is_some(), measure);
    let text_widths = scene
        .tabs
        .iter()
        .map(|(name, _)| measure.text_width(name, layout.text))
        .collect();
    let id = |n: usize| ids[n];
    Built {
        metrics,
        width,
        layout,
        text_widths,
        paints,
        shown: scene.shown.map(id),
        hover: match scene.hover {
            Hover::Nothing => Hit::Nothing,
            Hover::Tab(n) => Hit::Tab(id(n)),
            Hover::Slot(n) => Hit::Slot(id(n)),
            Hover::Add => Hit::Add,
            Hover::Chevron => Hit::Chevron,
            Hover::Fullscreen => Hit::Fullscreen,
            Hover::Go => Hit::Go,
            Hover::List => Hit::List,
        },
        drag: scene.drag.map(|(dragged, before)| Drag {
            id: id(dragged),
            before: before.map(id),
        }),
        bar: scene.bar.map(|b| BarPaint {
            text: b.text,
            focused: b.focused,
            error: b.error,
            edit_live: false,
        }),
        phase: scene.phase,
    }
}

impl Built {
    /// The draw list, the one the window runs.
    pub fn ops(&self, palette: &Palette) -> Vec<Op> {
        draw(&Paint {
            metrics: &self.metrics,
            palette,
            layout: &self.layout,
            width: self.width,
            tabs: &self.paints,
            shown: self.shown,
            hover: self.hover,
            drag: self.drag,
            bar: self.bar,
            phase: self.phase,
        })
    }
}
