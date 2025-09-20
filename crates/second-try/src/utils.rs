use regex::Regex;
use std::str::FromStr;
use std::time::{Duration, Instant};

pub fn get_numbers<T: FromStr>(source: &str) -> Vec<T>
where
    T::Err: std::fmt::Debug, /* add to toml
                             [dependencies]
                             regex = "1.3.9"
                             */
{
    // Use a regular expression to match the first sequence of digits in the string
    // Support negative and floating point numbers.
    let re = Regex::new(r"-?\d+(\.\d+)?").unwrap();
    let mut result: Vec<T> = vec![];
    for captures in re.captures_iter(source) {
        let digit_string = captures.get(0).unwrap().as_str();
        let number = digit_string.parse().unwrap();
        result.push(number);
    }
    return result;
}

#[derive(Debug, Clone, Copy)]
pub struct Timer {
    start: Instant,
    duration: Duration,
    running: bool,
}

impl Timer {
    pub fn new(running: bool) -> Timer {
        let start = Instant::now();
        Timer {
            start,
            duration: Duration::new(0, 0),
            running,
        }
    }

    pub fn start(&mut self) {
        if !self.running {
            self.start = Instant::now();
            self.running = true;
        }
    }

    pub fn stop(&mut self) -> Duration {
        if self.running == true {
            self.duration += Instant::now().duration_since(self.start);
            self.running = false;
        }
        self.duration
    }

    pub fn reset(&mut self, running: bool) {
        self.start = Instant::now();
        self.duration = Duration::new(0, 0);
        self.running = running;
    }

    pub fn elapsed(&self) -> Duration {
        if self.running {
            self.duration + Instant::now().duration_since(self.start)
        } else {
            self.duration
        }
    }
}
