/*
 * This file is part of rust-gdb.
 *
 * rust-gdb is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * rust-gdb is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with rust-gdb.  If not, see <http://www.gnu.org/licenses/>.
 */

extern crate regex;

mod dbg;
mod msg;
mod parser;

#[test]
fn start_debugger() {
    let mut dbg = dbg::Debugger::start().unwrap();
    let resp = dbg.send_cmd_raw("-break-info\n").unwrap();
    assert_eq!(msg::ResultClass::Done, resp.class);
}

#[test]
fn parse_stuff() {
    let resp = parser::parse_line("789^done,this=\"that\"\n").unwrap();
    match resp {
        msg::Record::Result(msg) => {
            println!("{:?}", msg);
        }
        _ => panic!("wrong type :("),
    };

    let resp = parser::parse_line("=stopped,this=\"that\"\n").unwrap();
    match resp {
        msg::Record::Async(msg) => {
            println!("{:?}", msg);
        }
        _ => panic!("wrong type :("),
    };
    let resp = parser::parse_line("~\"yadda yadda\"\n").unwrap();
    match resp {
        msg::Record::Stream(msg) => {
            println!("{:?}", msg);
        }
        _ => panic!("wrong type :("),
    };
}

pub use dbg::*;
pub use msg::*;
