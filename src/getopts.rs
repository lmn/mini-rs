// Copyright 2012-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//
// ignore-lexer-test FIXME #15677

//! Simple getopt alternative.
//!
//! Construct a vector of options, either by using `reqopt`, `optopt`, and
//! `optflag` or by building them from components yourself, and pass them to
//! `getopts`, along with a vector of actual arguments (not including
//! `argv[0]`). You'll either get a failure code back, or a match. You'll have
//! to verify whether the amount of 'free' arguments in the match is what you
//! expect. Use `opt_*` accessors to get argument values out of the matches
//! object.
//!
//! Single-character options are expected to appear on the command line with a
//! single preceding dash; multiple-character options are expected to be
//! proceeded by two dashes. Options that expect an argument accept their
//! argument following either a space or an equals sign. Single-character
//! options don't require the space.
//!
//! # Usage
//!
//! This crate is [on crates.io](https://crates.io/crates/getopts) and can be
//! used by adding `getopts` to the dependencies in your project's `Cargo.toml`.
//!
//! ```toml
//! [dependencies]
//! getopts = "0.2"
//! ```
//!
//! and this to your crate root:
//!
//! ```rust
//! extern crate mini;
//! ```
//!
//! # Example
//!
//! The following example shows simple command line parsing for an application
//! that requires an input file to be specified, accepts an optional output file
//! name following `-o`, and accepts both `-h` and `--help` as optional flags.
//!
//! ```{.rust}
//! extern crate mini;
//! use mini::getopts::Options;
//! use std::env;
//!
//! fn do_work(inp: &str, out: Option<String>) {
//!     println!("{}", inp);
//!     match out {
//!         Some(x) => println!("{}", x),
//!         None => println!("No Output"),
//!     }
//! }
//!
//! fn print_usage(program: &str, opts: Options) {
//!     let brief = format!("Usage: {} FILE [options]", program);
//!     print!("{}", opts.usage(&brief));
//! }
//!
//! fn main() {
//!     let args: Vec<String> = env::args().collect();
//!     let program = args[0].clone();
//!
//!     let mut opts = Options::new();
//!     opts.optopt("o", "", "set output file name", "NAME");
//!     opts.optflag("h", "help", "print this help menu");
//!     let matches = match opts.parse(&args[1..]) {
//!         Ok(m) => { m }
//!         Err(f) => { panic!(f.to_string()) }
//!     };
//!     if matches.opt_present("h") {
//!         print_usage(&program, opts);
//!         return;
//!     }
//!     let output = matches.opt_str("o");
//!     let input = if !matches.free.is_empty() {
//!         matches.free[0].clone()
//!     } else {
//!         print_usage(&program, opts);
//!         return;
//!     };
//!     do_work(&input, output);
//! }
//! ```

#![deny(missing_docs)]

use self::Name::*;
use self::HasArg::*;
use self::Occur::*;
use self::Fail::*;
use self::Optval::*;
use self::SplitWithinState::*;
use self::Whitespace::*;
use self::LengthLimit::*;

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::iter::{repeat, IntoIterator};
use std::result;

/// A description of the options that a program can handle.
pub struct Options {
    grps: Vec<OptGroup>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            grps: Vec::new(),
        }
    }
}

impl Options {
    /// Creates a blank set of options.
    pub fn new() -> Options {
        Self::default()
    }

    /// Creates a generic option group, stating all parameters explicitly.
    pub fn opt(&mut self, short_name: &str, long_name: &str, desc: &str,
                       hint: &str, hasarg: HasArg, occur: Occur) -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: hint.to_string(),
            desc: desc.to_string(),
            hasarg,
            occur
        });
        self
    }

    /// Creates an option that is optional and does not take an argument.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    pub fn optflag(&mut self, short_name: &str, long_name: &str, desc: &str)
                           -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: "".to_string(),
            desc: desc.to_string(),
            hasarg: No,
            occur: Optional
        });
        self
    }

    /// Creates a long option that can occur more than once and does not
    /// take an argument.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    pub fn optflagmulti(&mut self, short_name: &str, long_name: &str, desc: &str)
                                -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: "".to_string(),
            desc: desc.to_string(),
            hasarg: No,
            occur: Multi
        });
        self
    }

    /// Creates a long option that is optional and takes an optional argument.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    /// * `hint` - Hint that is used in place of the argument in the usage help,
    ///   e.g. `"FILE"` for a `-o FILE` option
    pub fn optflagopt(&mut self, short_name: &str, long_name: &str, desc: &str,
                              hint: &str) -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: hint.to_string(),
            desc: desc.to_string(),
            hasarg: Maybe,
            occur: Optional
        });
        self
    }

    /// Creates a long option that is optional, takes an argument, and may occur
    /// multiple times.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    /// * `hint` - Hint that is used in place of the argument in the usage help,
    ///   e.g. `"FILE"` for a `-o FILE` option
    pub fn optmulti(&mut self, short_name: &str, long_name: &str, desc: &str, hint: &str)
                            -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: hint.to_string(),
            desc: desc.to_string(),
            hasarg: Yes,
            occur: Multi
        });
        self
    }

    /// Creates a long option that is optional and takes an argument.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    /// * `hint` - Hint that is used in place of the argument in the usage help,
    ///   e.g. `"FILE"` for a `-o FILE` option
    pub fn optopt(&mut self, short_name: &str, long_name: &str, desc: &str, hint: &str)
                          -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: hint.to_string(),
            desc: desc.to_string(),
            hasarg: Yes,
            occur: Optional
        });
        self
    }

    /// Creates a long option that is required and takes an argument.
    ///
    /// * `short_name` - e.g. `"h"` for a `-h` option, or `""` for none
    /// * `long_name` - e.g. `"help"` for a `--help` option, or `""` for none
    /// * `desc` - Description for usage help
    /// * `hint` - Hint that is used in place of the argument in the usage help,
    ///   e.g. `"FILE"` for a `-o FILE` option
    pub fn reqopt(&mut self, short_name: &str, long_name: &str, desc: &str, hint: &str)
                          -> &mut Options {
        validate_names(short_name, long_name);
        self.grps.push(OptGroup {
            short_name: short_name.to_string(),
            long_name: long_name.to_string(),
            hint: hint.to_string(),
            desc: desc.to_string(),
            hasarg: Yes,
            occur: Req
        });
        self
    }

    /// Parses command line arguments according to the provided options.
    ///
    /// On success returns `Ok(Matches)`. Use methods such as `opt_present`
    /// `opt_str`, etc. to interrogate results.
    /// # Panics
    ///
    /// Returns `Err(Fail)` on failure: use the `Debug` implementation of `Fail`
    /// to display information about it.
    pub fn parse<C: IntoIterator>(&self, args: C) -> Result
        where C::Item: AsRef<OsStr>
    {
        let opts: Vec<Opt> = self.grps.iter().map(|x| x.long_to_short()).collect();

        let mut vals = (0 .. opts.len()).map(|_| Vec::new()).collect::<Vec<Vec<Optval>>>();
        let mut free: Vec<String> = Vec::new();
        let args = args.into_iter().map(|i| {
            i.as_ref().to_str().ok_or_else(|| {
                Fail::UnrecognizedOption(format!("{:?}", i.as_ref()))
            }).map(|s| s.to_owned())
        }).collect::<::std::result::Result<Vec<_>,_>>()?;
        let mut args = args.into_iter().peekable();
        while let Some(cur) = args.next() {
            if !is_arg(&cur) {
                // If it's not an argument starting with `-`, it's a free argument.
                free.push(cur);
            } else if cur == "--" {
                // After `--`, the rest of the arguments are free arguments.
                free.extend(args);
                break;
            } else {
                let mut names;
                let mut i_arg = None;
                let mut was_long = true;
                if cur.as_bytes()[1] == b'-' {
                    // Parsing long argument.
                    let tail = &cur[2..];
                    let mut parts = tail.splitn(2, '=');
                    names = vec![Name::from_str(parts.next().unwrap())];
                    if let Some(rest) = parts.next() {
                        i_arg = Some(rest.to_string());
                    }
                } else {
                    // Parsing short argument.
                    was_long = false;
                    names = Vec::new();
                    for (j, ch) in cur.char_indices().skip(1) {
                        let opt = Short(ch);

                        /* In a series of potential options (eg. -aheJ), if we
                           see one which takes an argument, we assume all
                           subsequent characters make up the argument. This
                           allows options such as -L/usr/local/lib/foo to be
                           interpreted correctly
                        */

                        let opt_id = match find_opt(&opts, &opt) {
                          Some(id) => id,
                          None => return Err(UnrecognizedOption(opt.to_string()))
                        };

                        names.push(opt);

                        let arg_follows = match opts[opt_id].hasarg {
                            Yes | Maybe => true,
                            No => false
                        };

                        if arg_follows {
                            let next = j + ch.len_utf8();
                            if next < cur.len() {
                                i_arg = Some(cur[next..].to_string());
                                break;
                            }
                        }
                    }
                }
                let mut name_pos = 0;
                for nm in &names {
                    name_pos += 1;
                    let optid = match find_opt(&opts, &nm) {
                      Some(id) => id,
                      None => return Err(UnrecognizedOption(nm.to_string()))
                    };
                    match opts[optid].hasarg {
                      No => {
                        if name_pos == names.len() && i_arg.is_some() {
                            return Err(UnexpectedArgument(nm.to_string()));
                        }
                        vals[optid].push(Given);
                      }
                      Maybe => {
                        // Note that here we do not handle `--arg value`.
                        // This matches GNU getopt behavior; but also
                        // makes sense, because if this were accepted,
                        // then users could only write a "Maybe" long
                        // option at the end of the arguments.
                        if let Some(i_arg) = i_arg.take() {
                            vals[optid].push(Val(i_arg));
                        } else if was_long || name_pos < names.len() || args.peek().map_or(true, |n| is_arg(&n)) {
                            vals[optid].push(Given);
                        } else {
                            vals[optid].push(Val(args.next().unwrap()));
                        }
                      }
                      Yes => {
                        if let Some(i_arg) = i_arg.take() {
                            vals[optid].push(Val(i_arg));
                        } else if let Some(n) = args.next() {
                            vals[optid].push(Val(n));
                        } else {
                            return Err(ArgumentMissing(nm.to_string()));
                        }
                      }
                    }
                }
            }
        }
        debug_assert_eq!(vals.len(), opts.len());
        for (vals, opt) in vals.iter().zip(opts.iter()) {
            if opt.occur == Req && vals.is_empty() {
                return Err(OptionMissing(opt.name.to_string()));
            }
            if opt.occur != Multi && vals.len() > 1 {
                return Err(OptionDuplicated(opt.name.to_string()));
            }
        }
        Ok(Matches {
            opts,
            vals,
            free
        })
    }

    /// Derives a short one-line usage summary from a set of long options.
    pub fn short_usage(&self, program_name: &str) -> String {
        let mut line = format!("Usage: {} ", program_name);
        line.push_str(&self.grps.iter()
                           .map(format_option)
                           .collect::<Vec<String>>()
                           .join(" "));
        line
    }

    /// Derives a formatted message from a set of options.
    pub fn usage(&self, brief: &str) -> String {
        self.usage_with_format(|opts|
            format!("{}\n\nOptions:\n{}\n", brief, opts.collect::<Vec<String>>().join("\n")))
    }

    /// Derives a custom formatted message from a set of options. The formatted options provided to
    /// a closure as an iterator.
    pub fn usage_with_format<F: FnMut(&mut Iterator<Item=String>) -> String>(&self, mut formatter: F) -> String {
        formatter(&mut self.usage_items())
    }

    /// Derives usage items from a set of options.
    fn usage_items<'a>(&'a self) -> Box<Iterator<Item=String> + 'a> {
        let desc_sep = format!("\n{}", repeat(" ").take(24).collect::<String>());

        let any_short = self.grps.iter().any(|optref| {
            !optref.short_name.is_empty()
        });

        let rows = self.grps.iter().map(move |optref| {
            let OptGroup{short_name,
                         long_name,
                         hint,
                         desc,
                         hasarg,
                         ..} = (*optref).clone();

            let mut row = "    ".to_string();

            // short option
            match short_name.len() {
                0 => {
                    if any_short {
                        row.push_str("    ");
                    }
                }
                1 => {
                    row.push('-');
                    row.push_str(&short_name);
                    if !long_name.is_empty() {
                        row.push_str(", ");
                    } else {
                        // Only a single space here, so that any
                        // argument is printed in the correct spot.
                        row.push(' ');
                    }
                }
                _ => panic!("the short name should only be 1 ascii char long"),
            }

            // long option
            match long_name.len() {
                0 => {}
                _ => {
                    row.push_str("--");
                    row.push_str(&long_name);
                    row.push(' ');
                }
            }

            // arg
            match hasarg {
                No => {}
                Yes => row.push_str(&hint),
                Maybe => {
                    row.push('[');
                    row.push_str(&hint);
                    row.push(']');
                }
            }

            // FIXME: #5516 should be graphemes not codepoints
            // here we just need to indent the start of the description
            let rowlen = row.chars().count();
            if rowlen < 24 {
                for _ in 0 .. 24 - rowlen {
                    row.push(' ');
                }
            } else {
                row.push_str(&desc_sep)
            }

            // Normalize desc to contain words separated by one space character
            let mut desc_normalized_whitespace = String::new();
            for word in desc.split(|c: char| c.is_whitespace())
                            .filter(|s| !s.is_empty()) {
                desc_normalized_whitespace.push_str(word);
                desc_normalized_whitespace.push(' ');
            }

            // FIXME: #5516 should be graphemes not codepoints
            let mut desc_rows = Vec::new();
            each_split_within(&desc_normalized_whitespace,
                              54,
                              |substr| {
                desc_rows.push(substr.to_string());
                true
            });

            // FIXME: #5516 should be graphemes not codepoints
            // wrapped description
            row.push_str(&desc_rows.join(&desc_sep));

            row
        });

       Box::new(rows)
    }
}

fn validate_names(short_name: &str, long_name: &str) {
    let len = short_name.len();
    assert!(len == 1 || len == 0,
            "the short_name (first argument) should be a single character, \
             or an empty string for none");
    let len = long_name.len();
    assert!(len == 0 || len > 1,
            "the long_name (second argument) should be longer than a single \
             character, or an empty string for none");
}

/// Name of an option. Either a string or a single char.
#[derive(Clone, PartialEq, Eq)]
enum Name {
    /// A string representing the long name of an option.
    /// For example: "help"
    Long(String),
    /// A char representing the short name of an option.
    /// For example: 'h'
    Short(char),
}

/// Describes whether an option has an argument.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HasArg {
    /// The option requires an argument.
    Yes,
    /// The option takes no argument.
    No,
    /// The option argument is optional.
    Maybe,
}

/// Describes how often an option may occur.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Occur {
    /// The option occurs once.
    Req,
    /// The option occurs at most once.
    Optional,
    /// The option occurs zero or more times.
    Multi,
}

/// A description of a possible option.
#[derive(Clone, PartialEq, Eq)]
struct Opt {
    /// Name of the option
    name: Name,
    /// Whether it has an argument
    hasarg: HasArg,
    /// How often it can occur
    occur: Occur,
    /// Which options it aliases
    aliases: Vec<Opt>,
}

/// One group of options, e.g., both `-h` and `--help`, along with
/// their shared description and properties.
#[derive(Clone, PartialEq, Eq)]
struct OptGroup {
    /// Short name of the option, e.g. `h` for a `-h` option
    short_name: String,
    /// Long name of the option, e.g. `help` for a `--help` option
    long_name: String,
    /// Hint for argument, e.g. `FILE` for a `-o FILE` option
    hint: String,
    /// Description for usage help text
    desc: String,
    /// Whether option has an argument
    hasarg: HasArg,
    /// How often it can occur
    occur: Occur
}

/// Describes whether an option is given at all or has a value.
#[derive(Clone, PartialEq, Eq)]
enum Optval {
    Val(String),
    Given,
}

/// The result of checking command line arguments. Contains a vector
/// of matches and a vector of free strings.
#[derive(Clone, PartialEq, Eq)]
pub struct Matches {
    /// Options that matched
    opts: Vec<Opt>,
    /// Values of the Options that matched
    vals: Vec<Vec<Optval>>,
    /// Free string fragments
    pub free: Vec<String>,
}

/// The type returned when the command line does not conform to the
/// expected format. Use the `Debug` implementation to output detailed
/// information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// The option requires an argument but none was passed.
    ArgumentMissing(String),
    /// The passed option is not declared among the possible options.
    UnrecognizedOption(String),
    /// A required option is not present.
    OptionMissing(String),
    /// A single occurrence option is being used multiple times.
    OptionDuplicated(String),
    /// There's an argument being passed to a non-argument option.
    UnexpectedArgument(String),
}

impl Error for Fail {
    fn description(&self) -> &str {
        match *self {
            ArgumentMissing(_) => "missing argument",
            UnrecognizedOption(_) => "unrecognized option",
            OptionMissing(_) => "missing option",
            OptionDuplicated(_) => "duplicated option",
            UnexpectedArgument(_) => "unexpected argument",
        }
    }
}

/// The result of parsing a command line with a set of options.
pub type Result = result::Result<Matches, Fail>;

impl Name {
    fn from_str(nm: &str) -> Name {
        if nm.len() == 1 {
            Short(nm.as_bytes()[0] as char)
        } else {
            Long(nm.to_string())
        }
    }

    fn to_string(&self) -> String {
        match *self {
            Short(ch) => ch.to_string(),
            Long(ref s) => s.to_string()
        }
    }
}

impl OptGroup {
    /// Translate OptGroup into Opt.
    /// (Both short and long names correspond to different Opts).
    fn long_to_short(&self) -> Opt {
        let OptGroup {
            short_name,
            long_name,
            hasarg,
            occur,
            ..
        } = (*self).clone();

        match (short_name.len(), long_name.len()) {
            (0,0) => panic!("this long-format option was given no name"),
            (0,_) => Opt {
                name: Long(long_name),
                hasarg,
                occur,
                aliases: Vec::new()
            },
            (1,0) => Opt {
                name: Short(short_name.as_bytes()[0] as char),
                hasarg,
                occur,
                aliases: Vec::new()
            },
            (1,_) => Opt {
                name: Long(long_name),
                hasarg,
                occur,
                aliases: vec!(
                    Opt {
                        name: Short(short_name.as_bytes()[0] as char),
                        hasarg: hasarg,
                        occur:  occur,
                        aliases: Vec::new()
                    }
                )
            },
            (_,_) => panic!("something is wrong with the long-form opt")
        }
    }
}

impl Matches {
    fn opt_vals(&self, nm: &str) -> Vec<Optval> {
        match find_opt(&self.opts, &Name::from_str(nm)) {
            Some(id) => self.vals[id].clone(),
            None => panic!("No option '{}' defined", nm)
        }
    }

    fn opt_val(&self, nm: &str) -> Option<Optval> {
        self.opt_vals(nm).into_iter().next()
    }
    /// Returns true if an option was defined
    pub fn opt_defined(&self, nm: &str) -> bool {
        find_opt(&self.opts, &Name::from_str(nm)).is_some()
    }

    /// Returns true if an option was matched.
    pub fn opt_present(&self, nm: &str) -> bool {
        !self.opt_vals(nm).is_empty()
    }

    /// Returns the number of times an option was matched.
    pub fn opt_count(&self, nm: &str) -> usize {
        self.opt_vals(nm).len()
    }

    /// Returns true if any of several options were matched.
    pub fn opts_present(&self, names: &[String]) -> bool {
        names.iter().any(|nm| {
            match find_opt(&self.opts, &Name::from_str(&nm)) {
                Some(id) if !self.vals[id].is_empty() => true,
                _ => false,
            }
        })
    }

    /// Returns the string argument supplied to one of several matching options or `None`.
    pub fn opts_str(&self, names: &[String]) -> Option<String> {
        names.iter().filter_map(|nm| {
            match self.opt_val(&nm) {
                Some(Val(s)) => Some(s),
                _ => None,
            }
        }).next()
    }

    /// Returns a vector of the arguments provided to all matches of the given
    /// option.
    ///
    /// Used when an option accepts multiple values.
    pub fn opt_strs(&self, nm: &str) -> Vec<String> {
        self.opt_vals(nm).into_iter().filter_map(|v| {
            match v {
                Val(s) => Some(s),
                _ => None,
            }
        }).collect()
    }

    /// Returns the string argument supplied to a matching option or `None`.
    pub fn opt_str(&self, nm: &str) -> Option<String> {
        match self.opt_val(nm) {
            Some(Val(s)) => Some(s),
            _ => None,
        }
    }


    /// Returns the matching string, a default, or `None`.
    ///
    /// Returns `None` if the option was not present, `def` if the option was
    /// present but no argument was provided, and the argument if the option was
    /// present and an argument was provided.
    pub fn opt_default(&self, nm: &str, def: &str) -> Option<String> {
        match self.opt_val(nm) {
            Some(Val(s)) => Some(s),
            Some(_) => Some(def.to_string()),
            None => None,
        }
    }

}

fn is_arg(arg: &str) -> bool {
    arg.as_bytes().get(0) == Some(&b'-') && arg.len() > 1
}

fn find_opt(opts: &[Opt], nm: &Name) -> Option<usize> {
    // Search main options.
    let pos = opts.iter().position(|opt| &opt.name == nm);
    if pos.is_some() {
        return pos
    }

    // Search in aliases.
    for candidate in opts.iter() {
        if candidate.aliases.iter().any(|opt| &opt.name == nm) {
            return opts.iter().position(|opt| opt.name == candidate.name);
        }
    }

    None
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ArgumentMissing(ref nm) => {
                write!(f, "Argument to option '{}' missing", *nm)
            }
            UnrecognizedOption(ref nm) => {
                write!(f, "Unrecognized option: '{}'", *nm)
            }
            OptionMissing(ref nm) => {
                write!(f, "Required option '{}' missing", *nm)
            }
            OptionDuplicated(ref nm) => {
                write!(f, "Option '{}' given more than once", *nm)
            }
            UnexpectedArgument(ref nm) => {
                write!(f, "Option '{}' does not take an argument", *nm)
            }
        }
    }
}

fn format_option(opt: &OptGroup) -> String {
    let mut line = String::new();

    if opt.occur != Req {
        line.push('[');
    }

    // Use short_name if possible, but fall back to long_name.
    if !opt.short_name.is_empty() {
        line.push('-');
        line.push_str(&opt.short_name);
    } else {
        line.push_str("--");
        line.push_str(&opt.long_name);
    }

    if opt.hasarg != No {
        line.push(' ');
        if opt.hasarg == Maybe {
            line.push('[');
        }
        line.push_str(&opt.hint);
        if opt.hasarg == Maybe {
            line.push(']');
        }
    }

    if opt.occur != Req {
        line.push(']');
    }
    if opt.occur == Multi {
        line.push_str("..");
    }

    line
}

#[derive(Clone, Copy)]
enum SplitWithinState {
    A,  // leading whitespace, initial state
    B,  // words
    C,  // internal and trailing whitespace
}

#[derive(Clone, Copy)]
enum Whitespace {
    Ws, // current char is whitespace
    Cr  // current char is not whitespace
}

#[derive(Clone, Copy)]
enum LengthLimit {
    UnderLim, // current char makes current substring still fit in limit
    OverLim   // current char makes current substring no longer fit in limit
}


/// Splits a string into substrings with possibly internal whitespace,
/// each of them at most `lim` bytes long, if possible. The substrings
/// have leading and trailing whitespace removed, and are only cut at
/// whitespace boundaries.
///
/// Note: Function was moved here from `std::str` because this module is the only place that
/// uses it, and because it was too specific for a general string function.
fn each_split_within<'a, F>(ss: &'a str, lim: usize, mut it: F)
                            -> bool where F: FnMut(&'a str) -> bool {
    // Just for fun, let's write this as a state machine:

    let mut slice_start = 0;
    let mut last_start = 0;
    let mut last_end = 0;
    let mut state = A;
    let mut fake_i = ss.len();
    let mut lim = lim;

    let mut cont = true;

    // if the limit is larger than the string, lower it to save cycles
    if lim >= fake_i {
        lim = fake_i;
    }

    let mut machine = |cont: &mut bool, state: &mut SplitWithinState, (i, c): (usize, char)| {
        let whitespace = if c.is_whitespace() { Ws }       else { Cr };
        let limit      = if (i - slice_start + 1) <= lim  { UnderLim } else { OverLim };

        *state = match (*state, whitespace, limit) {
            (A, Ws, _)        => { A }
            (A, Cr, _)        => { slice_start = i; last_start = i; B }

            (B, Cr, UnderLim) => { B }
            (B, Cr, OverLim)  if (i - last_start + 1) > lim => {
                // A single word has gone over the limit.  In this
                // case we just accept that the word will be too long.
                B
            }
            (B, Cr, OverLim)  => {
                *cont = it(&ss[slice_start..last_end]);
                slice_start = last_start;
                B
            }
            (B, Ws, UnderLim) => {
                last_end = i;
                C
            }
            (B, Ws, OverLim)  => {
                last_end = i;
                *cont = it(&ss[slice_start..last_end]);
                A
            }

            (C, Cr, UnderLim) => {
                last_start = i;
                B
            }
            (C, Cr, OverLim)  => {
                *cont = it(&ss[slice_start..last_end]);
                slice_start = i;
                last_start = i;
                last_end = i;
                B
            }
            (C, Ws, OverLim)  => {
                *cont = it(&ss[slice_start..last_end]);
                A
            }
            (C, Ws, UnderLim) => {
                C
            }
        };

        *cont
    };

    ss.char_indices().all(|x| machine(&mut cont, &mut state, x));

    // Let the automaton 'run out' by supplying trailing whitespace
    while cont && match state { B | C => true, A => false } {
        machine(&mut cont, &mut state, (fake_i, ' '));
        fake_i += 1;
    }
    cont
}

#[test]
fn test_split_within() {
    fn t(s: &str, i: usize, u: &[String]) {
        let mut v = Vec::new();
        each_split_within(s, i, |s| { v.push(s.to_string()); true });
        assert!(v.iter().zip(u.iter()).all(|(a,b)| a == b));
    }
    t("", 0, &[]);
    t("", 15, &[]);
    t("hello", 15, &["hello".to_string()]);
    t("\nMary had a little lamb\nLittle lamb\n", 15, &[
        "Mary had a".to_string(),
        "little lamb".to_string(),
        "Little lamb".to_string()
    ]);
    t("\nMary had a little lamb\nLittle lamb\n", ::std::usize::MAX,
        &["Mary had a little lamb\nLittle lamb".to_string()]);
}

#[cfg(test)]
mod tests {
    use super::{HasArg, Name, Occur, Opt, Options};

    #[test]
    fn test_long_to_short() {
        let mut short = Opt {
            name: Name::Long("banana".to_string()),
            hasarg: HasArg::Yes,
            occur: Occur::Req,
            aliases: Vec::new(),
        };
        short.aliases = vec!(Opt { name: Name::Short('b'),
                                hasarg: HasArg::Yes,
                                occur: Occur::Req,
                                aliases: Vec::new() });
        let mut opts = Options::new();
        opts.reqopt("b", "banana", "some bananas", "VAL");
        let verbose = &opts.grps[0];
        assert!(verbose.long_to_short() == short);
    }
}
