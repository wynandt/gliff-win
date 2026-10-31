//! Upstream's `paintable.rs` tests, verbatim in substance (`unwrap` becomes
//! `expect`), then the Windows additions.

use super::Filter;
use super::Viewport;
use super::clean_mode;
use super::layout;
use super::protocol_scale;

fn fit(view: (u32, u32), device: f64, width: f64, height: f64) -> super::Layout {
    layout(view, device, width, height).expect("a non-empty layout")
}

#[test]
fn doubles_when_the_widget_has_room() {
    let l = fit((2560, 1440), 2.0, 2560.0, 1440.0);
    assert_eq!(l.factor, 2.0);
    assert_eq!((l.x, l.y, l.width, l.height), (0.0, 0.0, 2560.0, 1440.0));
}

#[test]
fn fits_when_two_does_not_fit() {
    let l = fit((2560, 1440), 2.0, 2560.0, 1400.0);
    assert!((l.height - 1400.0).abs() < 0.001, "{l:?}");
    assert!((l.width - 2488.888888).abs() < 0.001, "{l:?}");
    assert_eq!(l.y, 0.0);
}

#[test]
fn shrinks_fractionally_when_too_small() {
    let l = fit((2560, 1440), 1.0, 1280.0, 1000.0);
    assert_eq!(l.factor, 0.5);
    assert_eq!((l.width, l.height), (1280.0, 720.0));
    assert_eq!((l.x, l.y), (0.0, 140.0));
}

#[test]
fn offsets_snap_to_device_pixels() {
    let l = fit((1001, 601), 2.0, 1000.0, 600.0);
    assert_eq!(l.x * 2.0, (l.x * 2.0).floor(), "{l:?}");
    assert_eq!(l.y * 2.0, (l.y * 2.0).floor(), "{l:?}");
}

#[test]
fn zoom_is_the_integer_factor_or_one() {
    assert_eq!(fit((2560, 1440), 2.0, 2560.0, 1440.0).zoom(), 2);
    assert_eq!(fit((2560, 1440), 2.0, 2560.0, 1400.0).zoom(), 1);
    assert_eq!(fit((2560, 1440), 1.0, 1280.0, 1000.0).zoom(), 1);
}

#[test]
fn ultrawide_mirror_fills_matching_window() {
    let l = fit((3840, 1080), 1.0, 5120.0, 1440.0);
    assert_eq!((l.width, l.height), (5120.0, 1440.0));
    assert_eq!((l.x, l.y), (0.0, 0.0));
}

#[test]
fn fractional_surface_scale_fills_the_window() {
    let l = fit((1920, 1080), 1.2, 1600.0, 900.0);
    assert_eq!((l.width, l.height), (1600.0, 900.0));
    assert_eq!((l.x, l.y), (0.0, 0.0));
    assert_eq!(l.zoom(), 1);
}

#[test]
fn nearly_exact_modes_keep_integer_sampling() {
    for (device, width, height) in [(1.0, 1601.0, 901.0), (1.25, 1287.0, 727.0)] {
        let l = fit((1600, 900), device, width, height);
        assert_eq!(l.factor, 1.0, "device {device}: {l:?}");
        assert_eq!(l.width * device, 1600.0, "device {device}: {l:?}");
        assert_eq!(l.height * device, 900.0, "device {device}: {l:?}");
    }
}

#[test]
fn mode_alignment_slack_tracks_fractional_scale() {
    for device in [1.125, 1.25, 1.4, 1.75, 1.8, 2.5, 3.875] {
        let physical = (1410, 796);
        let view = clean_mode(physical.0, physical.1, device as f32);
        let l = fit(view, device, 1410.5 / device, 796.25 / device);
        assert_eq!(l.factor, 1.0, "scale {device}, view {view:?}");
    }
}

#[test]
fn below_one_scale_preserves_drawing_and_pointer_coordinates() {
    let device = 0.8;
    let l = fit((1920, 1080), device, 2400.0, 1350.0);
    assert_eq!((l.width, l.height), (2400.0, 1350.0));
    assert_eq!(l.width / l.factor * device, 1920.0);
    assert_eq!(l.height / l.factor * device, 1080.0);
}

#[test]
fn empty_inputs_have_no_layout() {
    assert!(layout((0, 0), 2.0, 100.0, 100.0).is_none());
    assert!(layout((10, 10), 2.0, 0.0, 100.0).is_none());
}

// Upstream `hypr-ipc` clean_mode cases the slack test relies on.
#[test]
fn clean_mode_rounds_to_the_scale_numerator() {
    assert_eq!(
        clean_mode(1366, 768, 1.25),
        (1360, 760),
        "5/4 needs multiples of 10"
    );
    assert_eq!(
        clean_mode(1411, 797, 1.0),
        (1410, 796),
        "even for the encoder"
    );
    assert_eq!(clean_mode(3840, 2400, 2.0), (3840, 2400), "already clean");
    assert_eq!(
        clean_mode(1000, 1000, 1.0 / 3.0_f32.sqrt()),
        (1000, 1000),
        "no small q"
    );
    assert_eq!(clean_mode(2, 2, 1.25), (10, 10), "never below one step");
}

// The Windows additions.

#[test]
fn the_panel_at_two_hundred_percent_is_one_to_one_in_device_pixels() {
    // A 3840x2400 output in a 3840x2400 fullscreen client at 192 DPI.
    let device = f64::from(protocol_scale(192));
    let l = fit((3840, 2400), device, 3840.0 / device, 2400.0 / device);
    assert_eq!(l.factor, 1.0, "{l:?}");
    assert_eq!(
        l.viewport(device),
        Viewport {
            x: 0,
            y: 0,
            width: 3840,
            height: 2400
        }
    );
    assert_eq!(l.filter(), Filter::Point, "1:1 stays sharp");
}

#[test]
fn a_letterboxed_viewport_is_centred_in_device_pixels() {
    // 2560x1600 view in a 2560x1700 client at 2.0: 50 device px bars.
    let l = fit((2560, 1600), 2.0, 1280.0, 850.0);
    assert_eq!(
        l.viewport(2.0),
        Viewport {
            x: 0,
            y: 50,
            width: 2560,
            height: 1600
        },
        "{l:?}"
    );
    assert_eq!(l.filter(), Filter::Point);
}

#[test]
fn a_fractional_fit_samples_linearly() {
    let l = fit((2560, 1440), 2.0, 2560.0, 1400.0);
    assert_eq!(l.filter(), Filter::Linear, "{l:?}");
    let v = l.viewport(2.0);
    assert_eq!(v.height, 2800, "fills the height: {v:?}");
    assert_eq!(v.width, 4978, "2488.89 logical x 2: {v:?}");
}

#[test]
fn protocol_scale_is_dpi_over_96() {
    for (dpi, scale) in [
        (96, 1.0),
        (120, 1.25),
        (144, 1.5),
        (168, 1.75),
        (192, 2.0),
        (288, 3.0),
    ] {
        assert_eq!(protocol_scale(dpi), scale, "dpi {dpi}");
    }
}

mod to_remote {
    use super::super::to_remote;
    use crate::session::Geometry;

    fn geometry(stream: (u32, u32), view: (u32, u32), scale_milli: u32) -> Geometry {
        Geometry {
            stream,
            view,
            scale_milli,
        }
    }

    #[test]
    fn a_200_percent_window_maps_logical_pixels_to_remote_logical_pixels() {
        // 3840x2400 view at device scale 2, remote scale 2: the window is
        // 1920x1200 logical, the remote logical output is 1920x1200 too.
        let g = geometry((3840, 2400), (3840, 2400), 2000);
        let l = super::layout(g.view, 2.0, 1920.0, 1200.0).expect("layout");
        assert_eq!(
            to_remote(&l, &g, 2.0, 960.0, 600.0),
            (960.0, 600.0),
            "the middle"
        );
        assert_eq!(
            to_remote(&l, &g, 2.0, 1920.0, 1200.0),
            (1920.0, 1200.0),
            "the corner"
        );
    }

    #[test]
    fn a_150_percent_window_with_a_150_percent_remote_scale() {
        let g = geometry((3840, 2400), (3840, 2400), 1500);
        let l = super::layout(g.view, 1.5, 2560.0, 1600.0).expect("layout");
        let (x, y) = to_remote(&l, &g, 1.5, 1280.0, 800.0);
        assert!(
            (x - 1280.0).abs() < 1e-9 && (y - 800.0).abs() < 1e-9,
            "{x},{y}"
        );
    }

    #[test]
    fn letterbox_points_clamp_to_the_picture_edge() {
        // A 1920x1080 picture in a 3000x1080 window is centered.
        let g = geometry((1920, 1080), (1920, 1080), 1000);
        let l = super::layout(g.view, 1.0, 3000.0, 1080.0).expect("layout");
        assert_eq!(
            to_remote(&l, &g, 1.0, 0.0, 10.0),
            (0.0, 10.0),
            "left of the picture"
        );
        assert_eq!(
            to_remote(&l, &g, 1.0, 2999.0, 10.0),
            (1920.0, 10.0),
            "right of it"
        );
        assert_eq!(
            to_remote(&l, &g, 1.0, 3000.0, -50.0),
            (1920.0, 0.0),
            "above it"
        );
    }

    #[test]
    fn a_reduced_resolution_stream_is_stretched_to_the_view() {
        // The stream is half the view in each direction; the pointer still
        // reaches the far corner of the stream.
        let g = geometry((960, 540), (1920, 1080), 1000);
        let l = super::layout(g.view, 1.0, 1920.0, 1080.0).expect("layout");
        assert_eq!(to_remote(&l, &g, 1.0, 1920.0, 1080.0), (960.0, 540.0));
        assert_eq!(to_remote(&l, &g, 1.0, 960.0, 540.0), (480.0, 270.0));
    }

    #[test]
    fn empty_geometry_gives_the_origin() {
        let l = super::layout((1920, 1080), 1.0, 1920.0, 1080.0).expect("layout");
        for g in [
            geometry((0, 1080), (1920, 1080), 1000),
            geometry((1920, 0), (1920, 1080), 1000),
            geometry((1920, 1080), (0, 0), 1000),
        ] {
            assert_eq!(to_remote(&l, &g, 1.0, 5.0, 5.0), (0.0, 0.0), "{g:?}");
        }
    }

    #[test]
    fn a_zero_scale_is_floored_instead_of_dividing_by_zero() {
        let g = geometry((100, 100), (100, 100), 0);
        let l = super::layout(g.view, 1.0, 100.0, 100.0).expect("layout");
        let (x, _) = to_remote(&l, &g, 1.0, 1.0, 1.0);
        assert!(x.is_finite(), "scale 0 is floored to 0.01");
    }
}

mod client_to_remote {
    use super::super::client_to_remote;
    use crate::session::Geometry;

    fn geometry(view: (u32, u32), scale_milli: u32) -> Geometry {
        Geometry {
            stream: view,
            view,
            scale_milli,
        }
    }

    #[test]
    fn device_pixels_at_200_percent_are_remote_logical_pixels_at_the_remote_scale() {
        // A 2560x1600 view in a 2560x1600 client at 200%: the picture fills
        // the window, and the remote output is 1280x800 logical.
        let g = geometry((2560, 1600), 2000);
        assert_eq!(
            client_to_remote(&g, (2560, 1600), 192, (1280, 800)),
            Some((640.0, 400.0)),
            "the middle of the window is the middle of the remote output"
        );
    }

    #[test]
    fn a_window_without_area_has_no_position() {
        let g = geometry((1920, 1080), 1000);
        assert_eq!(client_to_remote(&g, (0, 0), 96, (5, 5)), None);
    }
}

mod remote_to_client {
    use super::super::client_to_remote;
    use super::super::remote_to_client;
    use crate::session::Geometry;

    #[test]
    fn it_inverts_client_to_remote_within_a_remote_pixel_at_each_scale() {
        for (dpi, scale_milli) in [(96, 1000), (144, 1500), (192, 2000)] {
            let g = Geometry {
                stream: (2560, 1600),
                view: (2560, 1600),
                scale_milli,
            };
            let client = (2560, 1600);
            for target in [(5.0, 5.0), (400.5, 300.25), (1200.0, 700.0)] {
                if let Some(point) = remote_to_client(&g, client, dpi, target) {
                    let back = client_to_remote(&g, client, dpi, point).expect("maps back");
                    assert!(
                        (back.0 - target.0).abs() <= 1.0 && (back.1 - target.1).abs() <= 1.0,
                        "{target:?} at {dpi} dpi went to {point:?} and back to {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_window_without_area_has_no_point() {
        let g = Geometry {
            stream: (2560, 1600),
            view: (2560, 1600),
            scale_milli: 2000,
        };
        assert_eq!(remote_to_client(&g, (0, 0), 192, (1.0, 1.0)), None);
    }
}
