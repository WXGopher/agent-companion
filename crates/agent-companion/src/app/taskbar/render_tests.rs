//! Exercise readout updates and sizing without creating desktop windows.

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{PlatformError, Rgb8Pixel};

use super::{Along, Chip, ChipSource, TaskOutcomes, TaskbarView};
use agent_companion_core::state::AgentTasks;

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(window: &MinimalSoftwareWindow) -> Option<Vec<Rgb8Pixel>> {
    let mut pixels = None;
    window.draw_if_needed(|renderer| {
        let size = window.size();
        let mut buffer = vec![Rgb8Pixel::default(); (size.width * size.height) as usize];
        renderer.render(&mut buffer, size.width as usize);
        pixels = Some(buffer);
    });
    pixels
}

#[test]
fn account_identity_is_visible_and_maximum_quota_fits_at_every_scale() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    bar.show();
    let quota_pixels = |pixels: &[Rgb8Pixel]| {
        pixels
            .iter()
            .filter(|pixel| {
                i32::from(pixel.g) - i32::from(pixel.r) > 50
                    && i32::from(pixel.g) - i32::from(pixel.b) > 30
            })
            .count()
    };
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        let mut primary_pixels = None;
        for source in [ChipSource::Codex, ChipSource::Dodex] {
            let chip = Chip {
                agent: Some(source),
                value: "100%*".into(),
                tier: "good",
                tasks: AgentTasks::default(),
                outcomes: TaskOutcomes::default(),
            };
            bar.set_chips(&[chip], Along::Vertical);
            bar.sync_scale(scale);
            let pixels = draw(&window).unwrap();
            if let Some(primary) = &primary_pixels {
                assert_ne!(
                    primary, &pixels,
                    "same quota still renders different C/D labels"
                );
            } else {
                primary_pixels = Some(pixels.clone());
            }
            let foreground = quota_pixels(&pixels);
            assert!(foreground > 0);

            // A wider reference must reveal no additional quota pixels: this
            // catches a clipped instance initial, percentage or failure star.
            let size = window.size();
            window.set_size(slint::PhysicalSize::new(
                size.width + (32.0 * scale).round() as u32,
                size.height,
            ));
            let reference = draw(&window).unwrap();
            assert_eq!(
                foreground,
                quota_pixels(&reference),
                "{source:?} at {scale}"
            );
            bar.sync_scale(scale);
            let _ = draw(&window);
        }
    }
}

#[test]
fn simultaneous_accounts_keep_their_task_rows_in_both_orientations_and_all_dpis() {
    use slint::Model;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    bar.show();
    let chips = [
        Chip {
            agent: Some(ChipSource::Codex),
            value: "100%*".into(),
            tier: "good",
            tasks: AgentTasks {
                running: 2,
                ..Default::default()
            },
            outcomes: TaskOutcomes::default(),
        },
        Chip {
            agent: Some(ChipSource::Dodex),
            value: "13%".into(),
            tier: "low",
            tasks: AgentTasks {
                pending: 1,
                done: 3,
                running: 0,
            },
            outcomes: TaskOutcomes {
                failed: 1,
                stopped: 1,
            },
        },
    ];
    for scale in [1.0_f32, 1.25, 1.5, 2.0] {
        for along in [Along::Vertical, Along::Horizontal] {
            bar.set_chips(&chips, along);
            bar.sync_scale(scale);
            let pixels = draw(&window).expect("orientation or DPI changes repaint both accounts");
            let logical = super::bar_size(&[chips[0].tasks, chips[1].tasks], along);
            assert_eq!(
                bar.physical_size(),
                (
                    (logical.0 * scale).round() as i32,
                    (logical.1 * scale).round() as i32,
                )
            );
            let primary = bar.ui.get_chips().row_data(0).unwrap();
            let secondary = bar.ui.get_chips().row_data(1).unwrap();
            assert_eq!(
                (primary.agent.as_str(), primary.value.as_str()),
                ("codex", "100%*")
            );
            assert_eq!(
                (secondary.agent.as_str(), secondary.value.as_str()),
                ("dodex", "13%")
            );
            assert_eq!(
                (
                    primary.pending,
                    primary.running,
                    primary.done,
                    primary.failed
                ),
                (0, 2, 0, 0)
            );
            assert_eq!(
                (
                    secondary.pending,
                    secondary.running,
                    secondary.done,
                    secondary.failed
                ),
                (1, 0, 3, 1)
            );
            assert!(
                pixels
                    .iter()
                    .any(|pixel| i32::from(pixel.r) - i32::from(pixel.g) > 20),
                "Dodex's low quota is visible"
            );
            if let Some(dir) = agent_companion_core::compat::var_os("AGENT_COMPANION_RENDER_DIR") {
                let dir = std::path::PathBuf::from(dir);
                std::fs::create_dir_all(&dir).unwrap();
                let size = window.size();
                let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
                bytes.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
                let orientation = if along.is_vertical() {
                    "vertical"
                } else {
                    "horizontal"
                };
                std::fs::write(
                    dir.join(format!("dual-readout-{orientation}-{scale}.ppm")),
                    bytes,
                )
                .unwrap();
            }

            bar.set_chips(&chips, along);
            assert!(
                draw(&window).is_none(),
                "unchanged dual readouts schedule no frame"
            );
        }
    }
}

#[test]
fn idle_readout_rescales_its_pixels_and_repairs_size_without_changing_chips() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    let chips = [Chip {
        agent: Some(ChipSource::Codex),
        outcomes: TaskOutcomes::default(),
        value: "72%".into(),
        tier: "good",
        tasks: AgentTasks {
            done: 1,
            ..Default::default()
        },
    }];
    bar.show();
    bar.set_chips(&chips, Along::Vertical);
    let mut dot_at_100 = 0;
    for scale in [1.0_f32, 1.25, 1.5, 2.0, 1.0, 1.5] {
        bar.sync_scale(scale);
        assert_eq!(window.scale_factor(), scale);
        assert_eq!(
            bar.physical_size(),
            ((67.0 * scale).round() as i32, (39.0 * scale).round() as i32)
        );
        let pixels = draw(&window).expect("DPI changes repaint idle content");
        if let Some(dir) = agent_companion_core::compat::var_os("AGENT_COMPANION_RENDER_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            let size = window.size();
            let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
            bytes.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
            std::fs::write(dir.join(format!("readout-{scale}.ppm")), bytes).unwrap();
        }
        let size = window.size();
        let green = pixels
            .iter()
            .enumerate()
            // Measure only the quota-leading dot, excluding its green quota
            // text and the completed task line below it.
            .filter(|(index, pixel)| {
                let x = (*index % size.width as usize) as f32;
                let y = (*index / size.width as usize) as f32;
                x < 12.0 * scale
                    && y < 19.0 * scale
                    && i32::from(pixel.g) - i32::from(pixel.r) > 50
                    && i32::from(pixel.g) - i32::from(pixel.b) > 30
            })
            .count();
        if scale == 1.0 {
            dot_at_100 = green;
            assert!(dot_at_100 > 0);
        } else {
            // Enlarging only the native window leaves the dot at its old
            // pixel size. Verify the rendered content grows with the DPI too.
            let ratio = green as f32 / dot_at_100 as f32;
            assert!(
                (ratio - scale * scale).abs() < 0.6,
                "dot area ratio {ratio}"
            );
        }
        bar.set_chips(&chips, Along::Vertical);
        bar.sync_scale(scale);
        bar.breathe(0.25);
        assert!(draw(&window).is_none(), "unchanged DPI stays idle");
    }

    // Windows can resize the child without changing Slint's scale.
    window.set_size(slint::PhysicalSize::new(45, 39));
    let _ = draw(&window);
    bar.sync_scale(1.5);
    assert_eq!(bar.physical_size(), (101, 59));
    assert!(draw(&window).is_some());
}

#[test]
fn task_status_colours_prioritize_waiting_and_breathe_only_while_active() {
    use slint::Model;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    bar.show();
    let rgb = |r, g, b| Rgb8Pixel { r, g, b };
    let blue = rgb(0x32, 0xb8, 0xff);
    let green = rgb(0x55, 0xc9, 0x8d);
    let yellow = rgb(0xe3, 0xbf, 0x52);
    let gray = rgb(0x6a, 0x6a, 0x80);
    let cases = [
        (AgentTasks::default(), gray),
        (
            AgentTasks {
                done: 2,
                ..Default::default()
            },
            green,
        ),
        (
            AgentTasks {
                running: 1,
                ..Default::default()
            },
            blue,
        ),
        (
            AgentTasks {
                running: 1,
                done: 2,
                ..Default::default()
            },
            blue,
        ),
        (
            AgentTasks {
                pending: 1,
                running: 1,
                done: 2,
            },
            yellow,
        ),
        (
            AgentTasks {
                pending: 1,
                done: 2,
                ..Default::default()
            },
            yellow,
        ),
    ];
    for agent in [ChipSource::Claude, ChipSource::Codex, ChipSource::Dodex] {
        for (tasks, expected) in cases {
            let chip = Chip {
                agent: Some(agent),
                outcomes: TaskOutcomes::default(),
                value: "72%".into(),
                tier: "good",
                tasks,
            };
            bar.set_chips(&[chip], Along::Vertical);
            bar.breathe(1.0);
            let bright = draw(&window).expect("a changed status repaints");
            let width = window.size().width as usize;
            let center = 11 * width + 7;
            assert_eq!(
                bright[center], expected,
                "quota dot for {agent:?}, {tasks:?}"
            );
            if tasks.done > 0 {
                assert!(
                    bright.iter().enumerate().any(|(index, pixel)| {
                        index / width >= 20
                            && i32::from(pixel.g) - i32::from(pixel.r) > 50
                            && i32::from(pixel.g) - i32::from(pixel.b) > 30
                    }),
                    "finished task mark and count are green"
                );
            }
            if tasks.running > 0 {
                assert!(
                    bright.iter().enumerate().any(|(index, pixel)| {
                        index / width >= 20 && i32::from(pixel.b) - i32::from(pixel.r) > 60
                    }),
                    "running task mark and count are blue, including Claude"
                );
            }
            bar.breathe(0.0);
            if tasks.active() > 0 {
                let low = draw(&window).expect("active status breathes");
                assert_eq!(
                    low[center],
                    rgb(0x92, 0x97, 0x9f),
                    "breaths reach opaque gray"
                );
            } else {
                assert!(
                    draw(&window).is_none(),
                    "idle and completed status stay still"
                );
            }
        }
    }
    let identities = [
        Chip {
            agent: Some(ChipSource::Codex),
            value: "72%".into(),
            outcomes: TaskOutcomes::default(),
            tier: "good",
            tasks: AgentTasks::default(),
        },
        Chip {
            agent: Some(ChipSource::Dodex),
            value: "31%".into(),
            outcomes: TaskOutcomes::default(),
            tier: "warn",
            tasks: AgentTasks::default(),
        },
    ];
    bar.set_chips(&identities, Along::Horizontal);
    let primary = bar.ui.get_chips().row_data(0).unwrap();
    let secondary = bar.ui.get_chips().row_data(1).unwrap();
    assert_eq!(
        (primary.agent.as_str(), primary.value.as_str()),
        ("codex", "72%")
    );
    assert_eq!(
        (secondary.agent.as_str(), secondary.value.as_str()),
        ("dodex", "31%")
    );
    let _ = draw(&window);
    bar.breathe(0.5);
    assert!(draw(&window).is_none(), "quotas without tasks remain idle");
}

#[test]
fn task_status_failed_and_stopped_outcomes_never_look_all_completed() {
    use slint::Model;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    bar.show();
    let rgb = |r, g, b| Rgb8Pixel { r, g, b };
    let blue = rgb(0x32, 0xb8, 0xff);
    let green = rgb(0x55, 0xc9, 0x8d);
    let yellow = rgb(0xe3, 0xbf, 0x52);
    let gray = rgb(0x6a, 0x6a, 0x80);
    let failed = TaskOutcomes {
        failed: 1,
        stopped: 0,
    };
    let stopped = TaskOutcomes {
        failed: 0,
        stopped: 1,
    };
    let finished = AgentTasks {
        done: 2,
        ..Default::default()
    };
    let cases = [
        (finished, TaskOutcomes::default(), green),
        (finished, failed, yellow),
        (finished, stopped, gray),
        (
            finished,
            TaskOutcomes {
                failed: 1,
                stopped: 1,
            },
            yellow,
        ),
        (
            AgentTasks {
                running: 1,
                ..finished
            },
            failed,
            blue,
        ),
        (
            AgentTasks {
                pending: 1,
                running: 1,
                ..finished
            },
            failed,
            yellow,
        ),
    ];
    for source in [ChipSource::Codex, ChipSource::Dodex] {
        for (tasks, outcomes, expected) in cases {
            bar.set_chips(
                &[Chip {
                    agent: Some(source),
                    value: "72%".into(),
                    tier: "good",
                    tasks,
                    outcomes,
                }],
                Along::Vertical,
            );
            bar.breathe(1.0);
            let bright =
                draw(&window).expect("changed outcomes repaint even when counts stay equal");
            let width = window.size().width as usize;
            assert_eq!(
                bright[11 * width + 7],
                expected,
                "{source:?}, {outcomes:?}, {tasks:?}"
            );
            assert_eq!(
                bar.ui.get_chips().row_data(0).unwrap().done,
                2,
                "lifecycle counter is retained"
            );
            if outcomes != TaskOutcomes::default() {
                assert!(
                    !bright.iter().enumerate().any(|(index, pixel)| {
                        index / width >= 20
                            && i32::from(pixel.g) - i32::from(pixel.r) > 50
                            && i32::from(pixel.g) - i32::from(pixel.b) > 30
                    }),
                    "unsuccessful terminal tasks do not get a green completion mark"
                );
            }
            bar.breathe(0.0);
            assert_eq!(
                draw(&window).is_some(),
                tasks.active() > 0,
                "only active tasks breathe"
            );
        }
    }
    if let Some(dir) = agent_companion_core::compat::var_os("AGENT_COMPANION_RENDER_DIR") {
        let preview = [
            (
                ChipSource::Codex,
                "run",
                AgentTasks {
                    running: 1,
                    ..Default::default()
                },
                TaskOutcomes::default(),
            ),
            (ChipSource::Dodex, "done", finished, TaskOutcomes::default()),
            (
                ChipSource::Codex,
                "wait",
                AgentTasks {
                    pending: 1,
                    ..Default::default()
                },
                TaskOutcomes::default(),
            ),
            (
                ChipSource::Dodex,
                "idle",
                AgentTasks::default(),
                TaskOutcomes::default(),
            ),
            (ChipSource::Codex, "fail", finished, failed),
            (ChipSource::Dodex, "stop", finished, stopped),
        ]
        .map(|(source, label, tasks, outcomes)| Chip {
            agent: Some(source),
            value: label.into(),
            tier: "",
            tasks,
            outcomes,
        });
        bar.set_chips(&preview, Along::Horizontal);
        bar.breathe(1.0);
        let pixels = draw(&window).unwrap();
        let size = window.size();
        let mut bytes = format!("P6\n{} {}\n255\n", size.width, size.height).into_bytes();
        bytes.extend(pixels.iter().flat_map(|pixel| [pixel.r, pixel.g, pixel.b]));
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("task-status-mixed.ppm"), bytes).unwrap();
    }
}

#[test]
fn readout_updates_colours_and_layout_without_scheduling_idle_frames() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(Rc::clone(&window)))).unwrap();
    let bar = TaskbarView::new(super::TaskbarBar::new().unwrap());
    let mut chips = vec![
        Chip {
            agent: Some(ChipSource::Claude),
            value: "23%".into(),
            outcomes: TaskOutcomes::default(),
            tier: "warn",
            tasks: AgentTasks::default(),
        },
        Chip {
            agent: Some(ChipSource::Codex),
            value: "34%".into(),
            outcomes: TaskOutcomes::default(),
            tier: "warn",
            tasks: AgentTasks::default(),
        },
    ];

    bar.show();
    for scale in [1.0, 1.25, 1.5, 2.0] {
        window.dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
        for along in [Along::Vertical, Along::Horizontal] {
            for tasks in [
                AgentTasks::default(),
                AgentTasks {
                    done: 2,
                    ..Default::default()
                },
            ] {
                chips[0].tasks = tasks;
                bar.set_chips(&chips, along);
                let clean = draw(&window).expect("changed content requests a frame");
                assert!(clean.iter().any(|pixel| pixel.r != pixel.b));
                let size = window.size();
                assert_eq!((size.width as i32, size.height as i32), bar.physical_size());

                bar.request_redraw();
                assert!(draw(&window).unwrap() == clean, "{scale}, {along:?}");

                // Unchanged data and finished sessions still schedule no frames.
                bar.set_chips(&chips, along);
                bar.breathe(0.25);
                assert!(draw(&window).is_none());
            }
        }
    }

    bar.request_redraw();
    let previous = draw(&window).unwrap();
    // Editing colour thresholds must refresh even if the number stays 23%.
    chips[0].tier = "low";
    bar.set_chips(&chips, Along::Horizontal);
    assert!(draw(&window).expect("a tier change requests a frame") != previous);

    bar.hide();
    let _ = draw(&window);
    bar.request_redraw();
    assert!(
        draw(&window).is_none(),
        "hidden readouts do not request frames"
    );
}
