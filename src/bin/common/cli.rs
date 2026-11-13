//! Command-line flag parsing shared by the binaries, which include this file
//! with `#[path]`.

/// Parses `-name value` and `-name=value` flags, with one or two dashes, up to
/// the first non-flag argument or `--`. A lone `-` is not a flag. Calls `set`
/// with each flag's name and value; `set` returns `None` for an unknown flag
/// or a bad value. Returns the remaining arguments, or `None` if a flag lacks
/// its value or `set` rejects it.
pub fn parse_flags(args: &[String], mut set: impl FnMut(&str, &str) -> Option<()>) -> Option<Vec<String>> {
    let mut i = 0;
    while i < args.len() && args[i].starts_with('-') && args[i].len() > 1 {
        if args[i] == "--" {
            i += 1;
            break;
        }
        let flag = args[i].trim_start_matches('-');
        i += 1;
        let (name, value) = match flag.split_once('=') {
            Some((name, value)) => (name, value),
            None => {
                i += 1;
                (flag, args.get(i - 1)?.as_str())
            }
        };
        set(name, value)?;
    }
    Some(args[i..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the flags as `name=value` strings and the remaining arguments.
    /// A flag named `bad` is rejected.
    fn parse(args: &[&str]) -> Option<(Vec<String>, Vec<String>)> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut flags = Vec::new();
        let rest = parse_flags(&args, |name, value| (name != "bad").then(|| flags.push(format!("{name}={value}"))))?;
        Some((flags, rest))
    }

    #[test]
    fn parse_flags_forms() {
        let (flags, rest) = parse(&["-a", "1", "--b", "2", "-c=3", "--d=", "x", "-e", "4"]).unwrap();
        assert_eq!(flags, ["a=1", "b=2", "c=3", "d="]);
        assert_eq!(rest, ["x", "-e", "4"]);
    }

    #[test]
    fn parse_flags_terminators() {
        assert_eq!(
            parse(&["-a", "1", "--", "-b", "2"]).unwrap(),
            (vec!["a=1".to_string()], vec!["-b".into(), "2".into()])
        );
        assert_eq!(parse(&["-", "-a", "1"]).unwrap().1, ["-", "-a", "1"]);
        assert_eq!(parse(&[]).unwrap(), (Vec::new(), Vec::new()));
    }

    #[test]
    fn parse_flags_errors() {
        assert_eq!(parse(&["-a"]), None);
        assert_eq!(parse(&["-bad", "1"]), None);
        assert_eq!(parse(&["-a", "1", "-bad=1", "x"]), None);
    }
}
