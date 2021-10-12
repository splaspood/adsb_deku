//! This tui program displays the current ADS-B detected airplanes on a plot with your current
//! position as (0,0) and has the ability to show different information about aircraft locations
//! and testing your coverage.
//!
//! # Tabs
//!
//! ## ADSB
//!
//! Regular display of recently observed aircraft on a lat/long plot
//!
//! ## Coverage
//!
//! Instead of only showing current airplanes, only plot dots for a seen airplane location
//!
//! Instead of using a HashMap for only storing an aircraft position for each aircraft, store
//! all aircrafts and only display a dot where detection at the lat/long position. This is for
//! testing the reach of your antenna.

use adsb_deku::deku::DekuContainerRead;
use adsb_deku::{Frame, DF, ME};

use std::io::{self, BufRead, BufReader};
use std::net::TcpStream;
use std::num::ParseFloatError;
use std::str::FromStr;
use std::time::Duration;

use apps::Airplanes;

use clap::{AppSettings, Clap};

use crossterm::event::{poll, read, Event, KeyCode, KeyEvent};
use crossterm::terminal::enable_raw_mode;

use tui::backend::{Backend, CrosstermBackend};
use tui::layout::{Constraint, Direction, Layout};
use tui::style::{Color, Style};
use tui::symbols::DOT;
use tui::text::Spans;
use tui::widgets::canvas::{Canvas, Line, Points};
use tui::widgets::{Block, Borders, Tabs};
use tui::Terminal;

/// Amount of zoom out from your original lat/long position
const MAX_PLOT_HIGH: f64 = 400.0;
const MAX_PLOT_LOW: f64 = MAX_PLOT_HIGH * -1.0;
/// Difference between each lat point
const LAT_DIFF: f64 = 1.2;
/// Difference between each long point
const LONG_DIFF: f64 = LAT_DIFF * 3.0;

pub struct City {
    name: String,
    lat: f64,
    long: f64,
}

impl FromStr for City {
    type Err = ParseFloatError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let coords: Vec<&str> = s
            .trim_matches(|p| p == '(' || p == ')')
            .split(',')
            .collect();

        let lat_fromstr = coords[1].parse::<f64>()?;
        let long_fromstr = coords[2].parse::<f64>()?;

        Ok(Self {
            name: coords[0].to_string(),
            lat: lat_fromstr,
            long: long_fromstr,
        })
    }
}

#[derive(Clap)]
#[clap(version = "1.0", author = "wcampbell <wcampbell1995@gmail.com>")]
#[clap(setting = AppSettings::ColoredHelp)]
struct Opts {
    /// Antenna location latitude
    #[clap(long)]
    lat: f64,
    /// Antenna location longitude
    #[clap(long)]
    long: f64,
    /// Vector of cities [(name, lat, long),..]
    #[clap(long)]
    cities: Vec<City>,
    /// Disable output of latitude and longitude on display
    #[clap(long)]
    disable_lat_long: bool,
}

#[derive(Copy, Clone)]
enum Tab {
    ADSB     = 0,
    Coverage = 1,
}

fn main() {
    let opts = Opts::parse();
    let local_lat = opts.lat;
    let local_long = opts.long;
    let cities = opts.cities;
    let disable_lat_long = opts.disable_lat_long;

    let stream = TcpStream::connect(("127.0.0.1", 30002)).unwrap();
    let mut reader = BufReader::new(stream);
    let mut input = String::new();
    let mut coverage_airplanes = vec![];
    let mut adsb_airplanes = Airplanes::new();

    let stdout = io::stdout();
    let mut backend = CrosstermBackend::new(stdout);
    backend.clear().unwrap();
    let mut terminal = Terminal::new(backend).unwrap();
    enable_raw_mode().unwrap();

    let mut tab_selection = Tab::ADSB;
    let mut quit = false;

    loop {
        let len = reader.read_line(&mut input).unwrap();
        let hex = &input.to_string()[1..len - 2];
        let bytes = hex::decode(&hex).unwrap();
        match Frame::from_bytes((&bytes, 0)) {
            Ok((_, frame)) => {
                if let DF::ADSB(ref adsb) = frame.df {
                    if let ME::AirbornePositionBaroAltitude(_) = adsb.me {
                        adsb_airplanes.add_extended_quitter_ap(adsb.icao, frame.clone());
                    }
                }
            }
            Err(_) => (),
        }

        // add lat_long to coverage vector
        let all_lat_long = adsb_airplanes.all_lat_long_altitude();
        coverage_airplanes.extend(all_lat_long);

        input.clear();
        // remove airplanes that timed-out
        adsb_airplanes.prune();

        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([Constraint::Percentage(5), Constraint::Percentage(95)].as_ref())
                    .split(f.size());

                let titles = ["ADSB", "Coverage"]
                    .iter()
                    .copied()
                    .map(Spans::from)
                    .collect();
                let tab = Tabs::new(titles)
                    .block(Block::default().title("Tabs").borders(Borders::ALL))
                    .style(Style::default().fg(Color::White))
                    .highlight_style(Style::default().fg(Color::Green))
                    .select(tab_selection as usize)
                    .divider(DOT);

                f.render_widget(tab, chunks[0]);

                match tab_selection {
                    Tab::ADSB => {
                        let canvas = Canvas::default()
                            .block(Block::default().title("ADSB").borders(Borders::ALL))
                            .x_bounds([MAX_PLOT_LOW, MAX_PLOT_HIGH])
                            .y_bounds([MAX_PLOT_LOW, MAX_PLOT_HIGH])
                            .paint(|ctx| {
                                ctx.layer();

                                // draw cities
                                for city in &cities {
                                    let lat = ((city.lat - local_lat) / LAT_DIFF) * MAX_PLOT_HIGH;
                                    let long =
                                        ((city.long - local_long) / LONG_DIFF) * MAX_PLOT_HIGH;

                                    // draw city coor
                                    ctx.draw(&Points {
                                        coords: &[(long, lat)],
                                        color: Color::Green,
                                    });

                                    // draw city name
                                    ctx.print(
                                        long + 3.0,
                                        lat,
                                        Box::leak(city.name.to_string().into_boxed_str()),
                                        Color::Green,
                                    );
                                }

                                // draw ADSB tab airplanes
                                for key in adsb_airplanes.0.keys() {
                                    let value = adsb_airplanes.lat_long_altitude(*key);
                                    if let Some((position, _altitude)) = value {
                                        let lat = ((position.latitude - local_lat) / LAT_DIFF)
                                            * MAX_PLOT_HIGH;
                                        let long = ((position.longitude - local_long) / LONG_DIFF)
                                            * MAX_PLOT_HIGH;

                                        // draw dot on location
                                        ctx.draw(&Points {
                                            coords: &[(long, lat)],
                                            color: Color::White,
                                        });

                                        let name = if !disable_lat_long {
                                            format!(
                                                "{} ({}, {})",
                                                key, position.latitude, position.longitude
                                            )
                                            .into_boxed_str()
                                        } else {
                                            format!("{}", key).into_boxed_str()
                                        };

                                        // draw plane ICAO name
                                        ctx.print(long + 3.0, lat, Box::leak(name), Color::White);
                                    }
                                }

                                // Draw vertical and horizontal lines
                                ctx.draw(&Line {
                                    x1: MAX_PLOT_HIGH,
                                    y1: 0.0,
                                    x2: MAX_PLOT_LOW,
                                    y2: 0.0,
                                    color: Color::White,
                                });
                                ctx.draw(&Line {
                                    x1: 0.0,
                                    y1: MAX_PLOT_HIGH,
                                    x2: 0.0,
                                    y2: MAX_PLOT_LOW,
                                    color: Color::White,
                                });
                            });
                        f.render_widget(canvas, chunks[1]);
                    }
                    Tab::Coverage => {
                        let canvas = Canvas::default()
                            .block(Block::default().title("Coverage").borders(Borders::ALL))
                            .x_bounds([MAX_PLOT_LOW, MAX_PLOT_HIGH])
                            .y_bounds([MAX_PLOT_LOW, MAX_PLOT_HIGH])
                            .paint(|ctx| {
                                ctx.layer();

                                // draw cities
                                for city in &cities {
                                    let lat = ((city.lat - local_lat) / LAT_DIFF) * MAX_PLOT_HIGH;
                                    let long =
                                        ((city.long - local_long) / LONG_DIFF) * MAX_PLOT_HIGH;

                                    // draw city coor
                                    ctx.draw(&Points {
                                        coords: &[(long, lat)],
                                        color: Color::Green,
                                    });

                                    // draw city name
                                    ctx.print(
                                        long + 3.0,
                                        lat,
                                        Box::leak(city.name.to_string().into_boxed_str()),
                                        Color::Green,
                                    );
                                }

                                // draw ADSB tab airplanes
                                for position in &coverage_airplanes {
                                    let lat = ((position.latitude - local_lat) / LAT_DIFF)
                                        * MAX_PLOT_HIGH;
                                    let long = ((position.longitude - local_long) / LONG_DIFF)
                                        * MAX_PLOT_HIGH;

                                    // draw dot on location
                                    ctx.draw(&Points {
                                        coords: &[(long, lat)],
                                        color: Color::White,
                                    });
                                }

                                // Draw vertical and horizontal lines
                                ctx.draw(&Line {
                                    x1: MAX_PLOT_HIGH,
                                    y1: 0.0,
                                    x2: MAX_PLOT_LOW,
                                    y2: 0.0,
                                    color: Color::White,
                                });
                                ctx.draw(&Line {
                                    x1: 0.0,
                                    y1: MAX_PLOT_HIGH,
                                    x2: 0.0,
                                    y2: MAX_PLOT_LOW,
                                    color: Color::White,
                                });
                            });
                        f.render_widget(canvas, chunks[1]);
                    }
                }
            })
            .unwrap();

        if poll(Duration::from_millis(100)).unwrap() {
            match read().unwrap() {
                Event::Key(KeyEvent { code, modifiers: _ }) => match code {
                    KeyCode::F(1) => tab_selection = Tab::ADSB,
                    KeyCode::F(2) => tab_selection = Tab::Coverage,
                    KeyCode::Char('q') => quit = true,
                    _ => (),
                },
                _ => (),
            }
        }
        if quit {
            break;
        }
    }
}
