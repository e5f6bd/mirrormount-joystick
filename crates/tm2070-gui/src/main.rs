use std::{any::TypeId, convert::Infallible, future::pending, ops::Range, time::Duration};

use anyhow::Context;
use iced::{
    alignment, event, font,
    futures::{channel::mpsc::Sender, SinkExt},
    keyboard::{self, Modifiers},
    mouse,
    widget::{
        button,
        canvas::{self, Cache, Frame, Geometry, Path, Program, Stroke, Text},
        column, row, slider, text, text_input, Canvas, Space,
    },
    window, Application, Color, Command, Event, Font, Length, Point, Rectangle, Renderer, Settings,
    Size, Subscription, Theme, Vector,
};
use itertools::{chain, iterate, zip_eq, Itertools};
use log::error;
use ordered_float::OrderedFloat;
use tm2070::Tm2070;
use tokio::{select, sync::mpsc::UnboundedSender};

const FONT: Font = Font::with_name("Noto Sans JP");

fn main() -> anyhow::Result<()> {
    env_logger::builder().format_timestamp_nanos().init();
    App::run(Settings {
        default_font: FONT,
        ..Default::default()
    })?;
    Ok(())
}

struct App {
    modifiers: Modifiers,

    com_port: String,
    connection_status: ConnectionStatus,

    average_count: usize,

    waveform_x: WaveformView,
    waveform_y: WaveformView,
    horizontal: WaveformHorizontalScale,

    status_message: String,
}
impl Default for App {
    fn default() -> Self {
        Self {
            modifiers: Default::default(),
            com_port: Default::default(),
            connection_status: Default::default(),
            average_count: 1,
            waveform_x: Default::default(),
            waveform_y: Default::default(),
            horizontal: Default::default(),
            status_message: Default::default(),
        }
    }
}

enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected(UnboundedSender<()>),
}
impl Default for ConnectionStatus {
    fn default() -> Self {
        Self::Disconnected
    }
}

impl Application for App {
    type Executor = iced::executor::Default;
    type Flags = ();
    type Message = Message;
    type Theme = Theme;

    fn new(_flags: Self::Flags) -> (Self, Command<Message>) {
        (
            Self::default(),
            Command::perform(
                fs_err::tokio::read("ignore/NotoSansJP-Regular.ttf"),
                |res| Message::FontFileLoaded(res.map_err(|e| e.to_string())),
            ),
        )
    }

    fn title(&self) -> String {
        "Auto collimator monitor".to_owned()
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Event(event) => match event {
                Event::Window(id, iced::window::Event::CloseRequested) => {
                    if let ConnectionStatus::Connected(tx) = &self.connection_status {
                        if let Err(e) = tx.send(()) {
                            self.status_message = format!("Failed to sopt: {e:#}");
                            log::error!("{}", self.status_message);
                        }
                    }
                    return window::close(id);
                }
                Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                    self.modifiers = modifiers
                }
                _ => (),
            },
            Message::FontFileLoaded(e) => match e {
                Ok(data) => return font::load(data).map(Message::FontLoaded),
                Err(e) => {
                    self.status_message = format!("Failed loading a font file (Japanese will not be displayed properly): {e:#?}");
                    log::error!("{}", self.status_message);
                }
            },
            Message::FontLoaded(e) => {
                if let Err(e) = e {
                    self.status_message = format!("Failed loading a font: {e:#?}");
                    log::error!("{}", self.status_message);
                }
            }
            Message::DataPoint(x, y) => {
                for (waveform, x) in zip_eq([&mut self.waveform_x, &mut self.waveform_y], [x, y]) {
                    waveform.points.push(x);
                    waveform
                        .cumulative_sum
                        .push(waveform.cumulative_sum.last().unwrap() + x);
                    waveform.waveform_frame_cache.clear();
                }
            }
            Message::ComPortInput(input) => self.com_port = input,
            Message::Connect => {
                self.connection_status = ConnectionStatus::Connecting;
                "Connecting...".clone_into(&mut self.status_message);
            }
            Message::ConnectionEstablished(tx) => {
                self.connection_status = ConnectionStatus::Connected(tx);
                "Connection established.".clone_into(&mut self.status_message);
            }
            Message::ConnectionLost(error_message) => {
                self.connection_status = ConnectionStatus::Disconnected;
                if let Some(message) = error_message {
                    self.status_message = message;
                }
            }
            Message::WaveformScrolled(position, window) => {
                self.horizontal.position = position;
                self.horizontal.window = window;
                for w in [&self.waveform_x, &self.waveform_y] {
                    w.waveform_frame_cache.clear();
                }
            }
            Message::SetAverageCount(count) => self.average_count = count,
        }
        Command::none()
    }

    fn view(&self) -> iced::Element<Message> {
        let canvas_x = Canvas::new(WaveformViewParam {
            modifiers: self.modifiers,
            view: &self.waveform_x,
            horizontal: self.horizontal,
            axis: WaveformViewAxis::X,
            average_count: self.average_count,
        })
        .width(Length::Fill)
        .height(Length::Fill);
        let canvas_y = Canvas::new(WaveformViewParam {
            modifiers: self.modifiers,
            view: &self.waveform_y,
            horizontal: self.horizontal,
            axis: WaveformViewAxis::Y,
            average_count: self.average_count,
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let config_line = {
            let com_port_label = text("COM port:");
            let disconnected = matches!(self.connection_status, ConnectionStatus::Disconnected);
            let com_port = text_input("e.g. COM3", &self.com_port);
            let com_port = if disconnected {
                com_port
                    .on_input(Message::ComPortInput)
                    .on_submit(Message::Connect)
            } else {
                com_port
            };
            let connect =
                button("Connect").on_press_maybe(disconnected.then_some(Message::Connect));

            let average_label = text(format!("Average: {}", self.average_count));
            let average_slider = slider(1. ..=60.001, self.average_count as f64, |a| {
                Message::SetAverageCount(a as usize)
            });

            row![
                com_port_label,
                com_port,
                connect,
                average_label,
                average_slider,
            ]
            .align_items(iced::Alignment::Center)
            .padding(5)
            .spacing(10)
        };

        let status_line = text(&self.status_message);

        let space = || Space::with_height(10);

        column![
            canvas_x,
            space(),
            canvas_y,
            space(),
            config_line,
            status_line
        ]
        .into()
    }

    fn subscription(&self) -> iced::Subscription<Self::Message> {
        struct SubscriptionId;

        let connected = !matches!(self.connection_status, ConnectionStatus::Disconnected);
        let connection = connected.then(|| {
            let com_port = self.com_port.clone();
            iced::subscription::channel(TypeId::of::<SubscriptionId>(), 100, |mut tx| async move {
                let Err(e) = tm2070_worker(&mut tx, com_port).await else {
                    // Ok(Infallible) never happens though...
                    pending().await
                };
                let error_message = format!("Connection to TM2070 lost or failed: {e:#}");
                error!("{error_message}");
                let _ = tx.send(Message::ConnectionLost(Some(error_message))).await;
                pending().await
            })
        });

        let events = event::listen().map(Message::Event);

        Subscription::batch(chain![connection, [events]])
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }
}

async fn tm2070_worker(tx: &mut Sender<Message>, com_port: String) -> anyhow::Result<Infallible> {
    let mut tm2070 =
        Tm2070::new(&com_port).with_context(|| format!("Could not connect to {com_port:?}"))?;
    let handle = tm2070.continuous_1(None)?;
    let (main_tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tx.send(Message::ConnectionEstablished(main_tx)).await?;
    loop {
        if let Some(event) = handle.recv()? {
            if let (Some(x), Some(y)) = (event.x, event.y) {
                tx.send(Message::DataPoint(x.value().val(), y.value().val()))
                    .await?;
            }
        }
        let sleep = tokio::time::sleep(Duration::from_secs_f64(1. / 120.));
        select! {
            _ = sleep => {},
            _ = rx.recv() => break,
        }
    }
    pending().await
}

struct WaveformViewParam<'a> {
    modifiers: Modifiers,
    #[allow(dead_code)]
    axis: WaveformViewAxis,
    view: &'a WaveformView,
    horizontal: WaveformHorizontalScale,
    average_count: usize,
}
#[derive(Clone, Copy, Debug)]
enum WaveformViewAxis {
    X,
    Y,
}

#[derive(Clone, Copy)]
struct WaveformHorizontalScale {
    position: WaveformPosition,
    // datapoints per full width
    window: f64,
}
impl Default for WaveformHorizontalScale {
    fn default() -> Self {
        Self {
            position: WaveformPosition::Rightmost,
            window: 600., // 10 seconds to fill
        }
    }
}

struct WaveformView {
    points: Vec<f64>,
    // f64 has 14-digit precision,
    // and the range of auto collimator is ~2e4 radians.
    // this allows ~2.6 years of measurement without losing precision.
    cumulative_sum: Vec<f64>,
    waveform_frame_cache: Cache,
}
impl Default for WaveformView {
    fn default() -> Self {
        Self {
            points: Default::default(),
            cumulative_sum: vec![0.],
            waveform_frame_cache: Default::default(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
enum WaveformPosition {
    Rightmost,
    Custom(f64),
}

impl Program<Message> for WaveformViewParam<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let geometry = self
            .view
            .waveform_frame_cache
            .draw(renderer, bounds.size(), |frame| {
                // Background
                frame.fill_rectangle(Point::ORIGIN, bounds.size(), Color::BLACK);

                let (x_range, to_x) = {
                    let left = self.leftmost_datapoint();
                    let right = left + self.horizontal.window;
                    let to = 0. ..bounds.width as f64;
                    let to_x = move |i| linear_map(i, left..right, to.clone()) as f32;
                    (left..right, to_x)
                };

                // Range of data indices that should be displayed.
                let (left, right) = (
                    // float -> int is saturating cast, so this never panics
                    x_range.start as usize,
                    (x_range.end as usize).min(self.num_points()),
                );
                let (left, right) = (left.saturating_sub(3), (right + 3).min(self.num_points()));

                let (y_range, to_y) = {
                    let points = || {
                        let points = &self.view.points[left..right];
                        points.iter().copied().map(OrderedFloat::from)
                    };
                    let from = match (points().min()).zip(points().max()) {
                        None => -1e-6..1e-6,
                        Some((min, max)) => {
                            let (min, max) = (min.into_inner(), max.into_inner());
                            if max - min > 2e-8 {
                                min..max
                            } else {
                                min - 1e-8..max + 1e-8
                            }
                        }
                    };
                    let to = {
                        let h = bounds.height as f64;
                        if h > 30. {
                            (h - 10.)..10.
                        } else {
                            (h * 2. / 3.)..h / 3.
                        }
                    };
                    let to_y = {
                        let from = from.clone();
                        let to = to.clone();
                        move |a| linear_map(a, from.clone(), to.clone()) as f32
                    };
                    (from, to_y)
                };

                // Grid, horizontal axis
                {
                    // let ws = [1., 2., 5., 10., 20., 50.].into_iter();
                    let ws = [0.6, 1.2, 3., 6., 12., 30.].into_iter();
                    let d_main =
                        grid_size(self.horizontal.window, bounds.width as _, 100., ws.clone())
                            .max(1.);
                    let d_sub =
                        grid_size(self.horizontal.window, bounds.width as _, 10., ws.clone())
                            .max(1.);

                    let iterate_i = |d: f64| iterate_range(d, x_range.clone());

                    for i in iterate_i(d_sub) {
                        let x = to_x(i);
                        let gray = 0.1;
                        frame.stroke(
                            &Path::line(Point::new(x, 0.), Point::new(x, bounds.height)),
                            Stroke::default().with_color(Color::from_rgb(gray, gray, gray)),
                        );
                    }

                    for i in iterate_i(d_main) {
                        let x = to_x(i);
                        let gray = 0.3;
                        frame.stroke(
                            &Path::line(Point::new(x, 0.), Point::new(x, bounds.height)),
                            Stroke::default().with_color(Color::from_rgb(gray, gray, gray)),
                        );
                        frame.fill_text(Text {
                            color: Color::WHITE,
                            size: 14.0.into(),
                            position: Point::new(x + 3., bounds.height - 3.),
                            horizontal_alignment: alignment::Horizontal::Left,
                            vertical_alignment: alignment::Vertical::Bottom,
                            content: format!("{i:.0}"),
                            ..Text::default()
                        });
                    }
                }

                // Grid, vertical axis
                {
                    let ws = [1., 2., 5., 10., 20., 50.].into_iter();
                    let range = y_range.end - y_range.start;
                    let d_main = grid_size(range, bounds.height as f64, 100., ws.clone());
                    let d_sub = grid_size(range, bounds.height as f64, 10., ws.clone());
                    let d_sub = d_sub
                        / if (d_sub / d_main).fract() > 0.3 {
                            2.
                        } else {
                            1.
                        };

                    let precision = {
                        let smallest_place = (d_main.log10() + 1e-5).floor() as isize;
                        (-smallest_place).max(0) as usize
                    };
                    let (exponent, unit) = match precision {
                        6.. => (6, "μrad"),
                        3.. => (3, "mrad"),
                        0.. => (0, "rad"),
                    };

                    let iterate_a = |d: f64| iterate_range(d, y_range.clone());

                    for a in iterate_a(d_sub) {
                        let y = to_y(a);
                        let gray = 0.1;
                        frame.stroke(
                            &Path::line(Point::new(0., y), Point::new(bounds.width, y)),
                            Stroke::default().with_color(Color::from_rgb(gray, gray, gray)),
                        );
                    }

                    for a in iterate_a(d_main) {
                        let y = to_y(a);
                        let gray = 0.3;
                        frame.stroke(
                            &Path::line(Point::new(0., y), Point::new(bounds.width, y)),
                            Stroke::default().with_color(Color::from_rgb(gray, gray, gray)),
                        );
                        frame.fill_text(Text {
                            color: Color::WHITE,
                            size: 14.0.into(),
                            position: Point::new(10., y - 3.),
                            horizontal_alignment: alignment::Horizontal::Left,
                            vertical_alignment: alignment::Vertical::Bottom,
                            content: format!(
                                "{:.precision$} {unit}",
                                a * 10f64.powi(exponent as _),
                                precision = precision - exponent,
                            ),
                            ..Text::default()
                        });
                    }
                }

                let show_rects = bounds.width as f64 / self.horizontal.window > 5.;

                // Plot
                let points =
                    (left..right).map(|i| Point::new(to_x(i as f64), to_y(self.view.points[i])));
                frame.stroke(
                    &path_from_iter(points.clone()),
                    Stroke::default().with_color(Color::from_rgba(0., 1., 0., 0.2)),
                );
                if show_rects {
                    draw_points(frame, points, Color::from_rgba(0., 1., 0., 0.2));
                }

                // Average plot
                // This involves computation, so we are creating result in vec
                let points = (left..right)
                    .filter_map(|i| {
                        let a = self.average_count;
                        let s = i.checked_sub(a / 2)?;
                        let t = i + a - a / 2;
                        let cum = &self.view.cumulative_sum;
                        let average = (cum.get(t)? - cum.get(s)?) / a as f64;
                        Some((i, average))
                    })
                    .map(|(i, point)| Point::new(to_x(i as f64), to_y(point)))
                    .collect_vec();
                frame.stroke(
                    &path_from_iter(points.iter().copied()),
                    Stroke::default().with_color(Color::from_rgba(1., 0.8, 0., 1.)),
                );
                if show_rects {
                    draw_points(frame, points, Color::from_rgba(1., 0.8, 0., 0.5));
                }
            });
        vec![geometry]
    }

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if cursor.position_in(bounds).is_some() {
            if let canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) = event {
                let (x, y) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => {
                        let line_to_pixel = 100.;
                        (x * line_to_pixel, y * line_to_pixel)
                    }
                    mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                let left_for_rightmost = self.left_for_rightmost();
                if self.modifiers.control() {
                    if let Some(position) = cursor.position() {
                        let left = self.leftmost_datapoint();
                        let right = left + self.horizontal.window;
                        let mid = linear_map(
                            position.x as f64,
                            bounds.x as f64..bounds.x as f64 + bounds.width as f64,
                            left..right,
                        );

                        let scale = 1.001f64.powf(-y as f64);
                        let new_left = mid - (mid - left) * scale;
                        let new_window = self.horizontal.window * scale;
                        let len = self.num_points() as f64;
                        let new_position = if len < new_window || len < new_left + new_window {
                            WaveformPosition::Rightmost
                        } else {
                            WaveformPosition::Custom(0f64.max(new_left))
                        };
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::WaveformScrolled(new_position, new_window)),
                        );
                    }
                } else {
                    // Scroll occurs only when data width is larger than displayed width
                    if self.left_for_rightmost() > 0. {
                        // Scroll delta on datapoint window
                        let delta_x = x as f64 * self.horizontal.window / bounds.width as f64;
                        let new_left = self.leftmost_datapoint() - delta_x;
                        let new_position = if left_for_rightmost < new_left {
                            WaveformPosition::Rightmost
                        } else {
                            WaveformPosition::Custom(0f64.max(new_left))
                        };
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::WaveformScrolled(
                                new_position,
                                self.horizontal.window,
                            )),
                        );
                    }
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }
}

impl WaveformViewParam<'_> {
    fn leftmost_datapoint(&self) -> f64 {
        match self.horizontal.position {
            WaveformPosition::Rightmost => 0f64.max(self.left_for_rightmost()),
            WaveformPosition::Custom(left) => left,
        }
    }

    // If this value < 0, it means the data point count is fewer than window.
    // In that case, the plot should be right-aligned.
    fn left_for_rightmost(&self) -> f64 {
        self.num_points() as f64 - self.horizontal.window
    }

    fn num_points(&self) -> usize {
        self.view.points.len()
    }
}

fn grid_size(
    range: f64,
    pixel_length: f64,
    h: f64,
    ws: impl DoubleEndedIterator<Item = f64> + Clone,
) -> f64 {
    // Sub grid height should be [h, h * 3), where h = 50 (heuristic)
    // Let grid step be d, then (height * d / range) in [h, h * 3)
    // d >= h * range / height =: k
    // d = {1, 2, 5} * 10^n
    // 10^[log10(k)] * {1, 2, 5}
    // 10^[log10(k) + 1] >= k
    let d1 = {
        let d_min = h * range / pixel_length;
        let pow10 = 10f64.powf(d_min.log10().floor());
        // let ws = [1., 2., 5., 10., 20., 50.].into_iter();
        ws.clone().map(|w| w * pow10).find(|&d| d >= d_min).unwrap()
    };
    // Also, there should be at least two gridlines.
    // range / d >= 2.2
    // d <= range / 2.2
    let d2 = {
        let d_max = range / 2.2;
        let pow10 = 10f64.powf(d_max.log10().floor() - 1.);
        // let ws = [1., 2., 5., 10., 20., 50.].into_iter().rev();
        ws.rev().map(|w| w * pow10).find(|&d| d <= d_max).unwrap()
    };
    d1.min(d2)
}

fn iterate_range(d: f64, range: Range<f64>) -> impl Iterator<Item = f64> {
    let start = (range.end / d).floor() * d;
    iterate(start, move |x| x - d).take_while(move |&a| a >= range.start)
}

fn linear_map(x: f64, from: Range<f64>, to: Range<f64>) -> f64 {
    to.start + (x - from.start) / (from.end - from.start) * (to.end - to.start)
}

fn path_from_iter(points: impl IntoIterator<Item = Point>) -> Path {
    let mut points = points.into_iter();
    Path::new(|builder| {
        if let Some(point) = points.by_ref().next() {
            builder.move_to(point);
        };
        for point in points {
            builder.line_to(point);
        }
    })
}

fn draw_points(frame: &mut Frame, points: impl IntoIterator<Item = Point>, color: Color) {
    for point in points {
        let size = 4.;
        frame.fill_rectangle(
            point - Vector::new(size, size) * 0.5,
            Size::new(size, size),
            color,
        );
    }
}

#[derive(Clone, Debug)]
enum Message {
    Event(Event),

    FontFileLoaded(Result<Vec<u8>, String>),
    FontLoaded(Result<(), iced::font::Error>),

    ComPortInput(String),
    Connect,
    ConnectionEstablished(UnboundedSender<()>),
    ConnectionLost(Option<String>),

    SetAverageCount(usize),

    WaveformScrolled(WaveformPosition, f64),

    DataPoint(f64, f64),
}
