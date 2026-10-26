//! Minimal Prometheus text exposition (format 0.0.4) shared by all services.
//!
//! Every service keeps plain atomic counters; this module only renders them. Metrics are
//! deliberately CPU-independent counters and gauges (dropped blocks, clients, error rates):
//! the host's own CPU / memory metrics belong to node-exporter or cAdvisor, not to Superhet.

use std::fmt::Write as _;

/// `Content-Type` of the exposition.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Kind of a metric family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Monotonically increasing value (reset only by a process restart).
    Counter,
    /// Value that can go up and down.
    Gauge,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
        }
    }
}

/// A text exposition under construction.
#[derive(Debug, Default)]
pub struct Exposition {
    out: String,
}

impl Exposition {
    /// An empty exposition.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a family: writes its `# HELP` and `# TYPE` lines. Call once per metric name,
    /// before its samples.
    pub fn family(&mut self, name: &str, help: &str, kind: Kind) -> &mut Self {
        let _ = writeln!(self.out, "# HELP {name} {}", escape_help(help));
        let _ = writeln!(self.out, "# TYPE {name} {}", kind.as_str());
        self
    }

    /// Add an integer sample of the family `name`.
    pub fn sample_u64(&mut self, name: &str, labels: &[(&str, &str)], value: u64) -> &mut Self {
        self.line(name, labels, &value.to_string())
    }

    /// Add a floating point sample of the family `name`; non-finite values are written as
    /// `NaN` / `+Inf` / `-Inf` as the format requires.
    pub fn sample_f64(&mut self, name: &str, labels: &[(&str, &str)], value: f64) -> &mut Self {
        let text = if value.is_nan() {
            "NaN".to_owned()
        } else if value.is_infinite() {
            if value > 0.0 { "+Inf" } else { "-Inf" }.to_owned()
        } else {
            value.to_string()
        };
        self.line(name, labels, &text)
    }

    /// Shorthand for a family with a single unlabelled integer sample.
    pub fn single_u64(&mut self, name: &str, help: &str, kind: Kind, value: u64) -> &mut Self {
        self.family(name, help, kind).sample_u64(name, &[], value)
    }

    /// Shorthand for a family with a single unlabelled float sample.
    pub fn single_f64(&mut self, name: &str, help: &str, kind: Kind, value: f64) -> &mut Self {
        self.family(name, help, kind).sample_f64(name, &[], value)
    }

    fn line(&mut self, name: &str, labels: &[(&str, &str)], value: &str) -> &mut Self {
        self.out.push_str(name);
        if !labels.is_empty() {
            self.out.push('{');
            for (i, (key, val)) in labels.iter().enumerate() {
                if i > 0 {
                    self.out.push(',');
                }
                let _ = write!(self.out, "{key}=\"{}\"", escape_label(val));
            }
            self.out.push('}');
        }
        self.out.push(' ');
        self.out.push_str(value);
        self.out.push('\n');
        self
    }

    /// The finished text.
    #[must_use]
    pub fn finish(self) -> String {
        self.out
    }
}

fn escape_help(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\n', "\\n")
}

fn escape_label(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_families_samples_and_labels() {
        let mut e = Exposition::new();
        e.single_u64("sh_up", "Process is up.", Kind::Gauge, 1);
        e.family("sh_x_total", "Things.", Kind::Counter)
            .sample_u64("sh_x_total", &[("kind", "a")], 3)
            .sample_f64("sh_x_total", &[("kind", "b"), ("dev", "r\"0")], 1.5);
        assert_eq!(
            e.finish(),
            "# HELP sh_up Process is up.\n# TYPE sh_up gauge\nsh_up 1\n\
             # HELP sh_x_total Things.\n# TYPE sh_x_total counter\n\
             sh_x_total{kind=\"a\"} 3\nsh_x_total{kind=\"b\",dev=\"r\\\"0\"} 1.5\n"
        );
    }

    #[test]
    fn non_finite_floats_use_prometheus_spelling() {
        let mut e = Exposition::new();
        e.sample_f64("a", &[], f64::NAN)
            .sample_f64("b", &[], f64::INFINITY)
            .sample_f64("c", &[], f64::NEG_INFINITY);
        assert_eq!(e.finish(), "a NaN\nb +Inf\nc -Inf\n");
    }
}
