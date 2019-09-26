#![allow(unused)]

use crate::parser::parse::{
    call_node::*, flag::*, operator::*, pipeline::*, token_tree::*, token_tree_builder::*,
    tokens::*, unit::*,
};
use crate::prelude::*;
use crate::{Tag, Tagged};
use nom;
use nom::branch::*;
use nom::bytes::complete::*;
use nom::character::complete::*;
use nom::combinator::*;
use nom::multi::*;
use nom::sequence::*;

use log::trace;
use nom::dbg;
use nom::*;
use nom::{AsBytes, FindSubstring, IResult, InputLength, InputTake, Slice};
use nom_locate::{position, LocatedSpanEx};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::str::FromStr;
use uuid::Uuid;

pub type NomSpan<'a> = LocatedSpanEx<&'a str, Uuid>;

pub fn nom_input(s: &str, origin: Uuid) -> NomSpan<'_> {
    LocatedSpanEx::new_extra(s, origin)
}

macro_rules! operator {
    ($name:tt : $token:tt ) => {
        pub fn $name(input: NomSpan) -> IResult<NomSpan, TokenNode> {
            let start = input.offset;
            let (input, tag) = tag(stringify!($token))(input)?;
            let end = input.offset;

            Ok((
                input,
                TokenTreeBuilder::tagged_op(tag.fragment, (start, end, input.extra)),
            ))
        }
    };
}

operator! { gt:  >  }
operator! { lt:  <  }
operator! { gte: >= }
operator! { lte: <= }
operator! { eq:  == }
operator! { neq: != }

fn trace_step<'a, T: Debug>(
    input: NomSpan<'a>,
    name: &str,
    block: impl FnOnce(NomSpan<'a>) -> IResult<NomSpan<'a>, T>,
) -> IResult<NomSpan<'a>, T> {
    trace!(target: "nu::lite_parse", "+ before {} @ {:?}", name, input);
    match block(input) {
        Ok((input, result)) => {
            trace!(target: "nu::lite_parse", "after {} @ {:?} -> {:?}", name, input, result);
            Ok((input, result))
        }

        Err(e) => {
            trace!(target: "nu::lite_parse", "- failed {} :: {:?}", name, e);
            Err(e)
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub enum Number {
    Int(BigInt),
    Decimal(BigDecimal),
}

macro_rules! primitive_int {
    ($($ty:ty)*) => {
        $(
            impl From<$ty> for Number {
                fn from(int: $ty) -> Number {
                    Number::Int(BigInt::zero() + int)
                }
            }

            impl From<&$ty> for Number {
                fn from(int: &$ty) -> Number {
                    Number::Int(BigInt::zero() + *int)
                }
            }
        )*
    }
}

primitive_int!(i8 u8 i16 u16 i32 u32 i64 u64 i128 u128);

macro_rules! primitive_decimal {
    ($($ty:tt -> $from:tt),*) => {
        $(
            impl From<$ty> for Number {
                fn from(decimal: $ty) -> Number {
                    Number::Decimal(BigDecimal::$from(decimal).unwrap())
                }
            }

            impl From<&$ty> for Number {
                fn from(decimal: &$ty) -> Number {
                    Number::Decimal(BigDecimal::$from(*decimal).unwrap())
                }
            }
        )*
    }
}

primitive_decimal!(f32 -> from_f32, f64 -> from_f64);

impl std::ops::Mul for Number {
    type Output = Number;

    fn mul(self, other: Number) -> Number {
        match (self, other) {
            (Number::Int(a), Number::Int(b)) => Number::Int(a * b),
            (Number::Int(a), Number::Decimal(b)) => Number::Decimal(BigDecimal::from(a) * b),
            (Number::Decimal(a), Number::Int(b)) => Number::Decimal(a * BigDecimal::from(b)),
            (Number::Decimal(a), Number::Decimal(b)) => Number::Decimal(a * b),
        }
    }
}

// For literals
impl std::ops::Mul<u32> for Number {
    type Output = Number;

    fn mul(self, other: u32) -> Number {
        match self {
            Number::Int(left) => Number::Int(left * (other as i64)),
            Number::Decimal(left) => Number::Decimal(left * BigDecimal::from(other)),
        }
    }
}

impl Into<Number> for BigDecimal {
    fn into(self) -> Number {
        Number::Decimal(self)
    }
}

pub fn raw_number(input: NomSpan) -> IResult<NomSpan, Tagged<RawNumber>> {
    let original = input;
    let start = input.offset;
    trace_step(input, "raw_decimal", move |input| {
        let (input, neg) = opt(tag("-"))(input)?;
        let (input, head) = digit1(input)?;
        let dot: IResult<NomSpan, NomSpan, (NomSpan, nom::error::ErrorKind)> = tag(".")(input);

        let input = match dot {
            Ok((input, dot)) => input,

            // it's just an integer
            Err(_) => return Ok((input, RawNumber::int((start, input.offset, input.extra)))),
        };

        let (input, tail) = digit1(input)?;

        let end = input.offset;

        Ok((input, RawNumber::decimal((start, end, input.extra))))
    })
}

pub fn operator(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "operator", |input| {
        let (input, operator) = alt((gte, lte, neq, gt, lt, eq))(input)?;

        Ok((input, operator))
    })
}

pub fn dq_string(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "dq_string", |input| {
        let start = input.offset;
        let (input, _) = char('"')(input)?;
        let start1 = input.offset;
        let (input, _) = many0(none_of("\""))(input)?;
        let end1 = input.offset;
        let (input, _) = char('"')(input)?;
        let end = input.offset;
        Ok((
            input,
            TokenTreeBuilder::tagged_string((start1, end1, input.extra), (start, end, input.extra)),
        ))
    })
}

pub fn sq_string(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "sq_string", move |input| {
        let start = input.offset;
        let (input, _) = char('\'')(input)?;
        let start1 = input.offset;
        let (input, _) = many0(none_of("\'"))(input)?;
        let end1 = input.offset;
        let (input, _) = char('\'')(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_string((start1, end1, input.extra), (start, end, input.extra)),
        ))
    })
}

pub fn string(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "string", move |input| {
        alt((sq_string, dq_string))(input)
    })
}

pub fn external(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "external", move |input| {
        let start = input.offset;
        let (input, _) = tag(";")(input)?;
        let (input, bare) = take_while(is_bare_char)(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_external(bare, (start, end, input.extra)),
        ))
    })
}

pub fn pattern(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "bare", move |input| {
        let start = input.offset;
        let (input, _) = take_while1(is_start_glob_char)(input)?;
        let (input, _) = take_while(is_glob_char)(input)?;

        let next_char = &input.fragment.chars().nth(0);

        if let Some(next_char) = next_char {
            if is_external_word_char(*next_char) {
                return Err(nom::Err::Error(nom::error::make_error(
                    input,
                    nom::error::ErrorKind::TakeWhile1,
                )));
            }
        }

        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_pattern((start, end, input.extra)),
        ))
    })
}

pub fn bare(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "bare", move |input| {
        let start = input.offset;
        let (input, _) = take_while1(is_start_bare_char)(input)?;
        let (input, _) = take_while(is_bare_char)(input)?;

        let next_char = &input.fragment.chars().nth(0);

        if let Some(next_char) = next_char {
            if is_external_word_char(*next_char) || is_glob_specific_char(*next_char) {
                return Err(nom::Err::Error(nom::error::make_error(
                    input,
                    nom::error::ErrorKind::TakeWhile1,
                )));
            }
        }

        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_bare((start, end, input.extra)),
        ))
    })
}

pub fn external_word(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "bare", move |input| {
        let start = input.offset;
        let (input, _) = take_while1(is_external_word_char)(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_external_word((start, end, input.extra)),
        ))
    })
}

pub fn var(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "var", move |input| {
        let start = input.offset;
        let (input, _) = tag("$")(input)?;
        let (input, bare) = member(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_var(bare.tag(), (start, end, input.extra)),
        ))
    })
}

pub fn member(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "identifier", move |input| {
        let start = input.offset;
        let (input, _) = take_while1(is_id_start)(input)?;
        let (input, _) = take_while(is_id_continue)(input)?;

        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_member((start, end, input.extra)),
        ))
    })
}

pub fn flag(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "flag", move |input| {
        let start = input.offset;
        let (input, _) = tag("--")(input)?;
        let (input, bare) = bare(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_flag(bare.tag(), (start, end, input.extra)),
        ))
    })
}

pub fn shorthand(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "shorthand", move |input| {
        let start = input.offset;
        let (input, _) = tag("-")(input)?;
        let (input, bare) = bare(input)?;
        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_shorthand(bare.tag(), (start, end, input.extra)),
        ))
    })
}

pub fn raw_unit(input: NomSpan) -> IResult<NomSpan, Tagged<Unit>> {
    trace_step(input, "raw_unit", move |input| {
        let start = input.offset;
        let (input, unit) = alt((
            tag("B"),
            tag("b"),
            tag("KB"),
            tag("kb"),
            tag("Kb"),
            tag("K"),
            tag("k"),
            tag("MB"),
            tag("mb"),
            tag("Mb"),
            tag("GB"),
            tag("gb"),
            tag("Gb"),
            tag("TB"),
            tag("tb"),
            tag("Tb"),
            tag("PB"),
            tag("pb"),
            tag("Pb"),
        ))(input)?;
        let end = input.offset;

        Ok((
            input,
            Unit::from(unit.fragment).tagged((start, end, input.extra)),
        ))
    })
}

pub fn size(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "size", move |input| {
        let mut is_size = false;
        let start = input.offset;
        let (input, number) = raw_number(input)?;
        if let Ok((input, Some(size))) = opt(raw_unit)(input) {
            let end = input.offset;

            // Check to make sure there is no trailing parseable characters
            if let Ok((input, Some(extra))) = opt(bare)(input) {
                return Err(nom::Err::Error((input, nom::error::ErrorKind::Char)));
            }

            Ok((
                input,
                TokenTreeBuilder::tagged_size((number.item, *size), (start, end, input.extra)),
            ))
        } else {
            let end = input.offset;

            // Check to make sure there is no trailing parseable characters
            if let Ok((input, Some(extra))) = opt(bare)(input) {
                return Err(nom::Err::Error((input, nom::error::ErrorKind::Char)));
            }

            Ok((
                input,
                TokenTreeBuilder::tagged_number(number.item, number.tag),
            ))
        }
    })
}

pub fn leaf(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "leaf", move |input| {
        let (input, node) = alt((
            size,
            string,
            operator,
            flag,
            shorthand,
            var,
            external,
            bare,
            pattern,
            external_word,
        ))(input)?;

        Ok((input, node))
    })
}

pub fn token_list(input: NomSpan) -> IResult<NomSpan, Vec<TokenNode>> {
    trace_step(input, "token_list", move |input| {
        let (input, first) = node(input)?;
        let (input, list) = many0(pair(space1, node))(input)?;

        Ok((input, make_token_list(None, first, list, None)))
    })
}

pub fn spaced_token_list(input: NomSpan) -> IResult<NomSpan, Vec<TokenNode>> {
    trace_step(input, "spaced_token_list", move |input| {
        let (input, sp_left) = opt(space1)(input)?;
        let (input, first) = node(input)?;
        let (input, list) = many0(pair(space1, node))(input)?;
        let (input, sp_right) = opt(space1)(input)?;

        Ok((input, make_token_list(sp_left, first, list, sp_right)))
    })
}

fn make_token_list(
    sp_left: Option<NomSpan>,
    first: TokenNode,
    list: Vec<(NomSpan, TokenNode)>,
    sp_right: Option<NomSpan>,
) -> Vec<TokenNode> {
    let mut nodes = vec![];

    if let Some(sp_left) = sp_left {
        nodes.push(TokenNode::Whitespace(Tag::from(sp_left)));
    }

    nodes.push(first);

    for (ws, token) in list {
        nodes.push(TokenNode::Whitespace(Tag::from(ws)));
        nodes.push(token);
    }

    if let Some(sp_right) = sp_right {
        nodes.push(TokenNode::Whitespace(Tag::from(sp_right)));
    }

    nodes
}

pub fn whitespace(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "whitespace", move |input| {
        let left = input.offset;
        let (input, ws1) = space1(input)?;
        let right = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_ws((left, right, input.extra)),
        ))
    })
}

pub fn delimited_paren(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "delimited_paren", move |input| {
        let left = input.offset;
        let (input, _) = char('(')(input)?;
        let (input, ws1) = opt(whitespace)(input)?;
        let (input, inner_items) = opt(token_list)(input)?;
        let (input, ws2) = opt(whitespace)(input)?;
        let (input, _) = char(')')(input)?;
        let right = input.offset;

        let mut items = vec![];

        if let Some(space) = ws1 {
            items.push(space);
        }

        if let Some(inner_items) = inner_items {
            items.extend(inner_items);
        }

        if let Some(space) = ws2 {
            items.push(space);
        }

        Ok((
            input,
            TokenTreeBuilder::tagged_parens(items, (left, right, input.extra)),
        ))
    })
}

pub fn delimited_square(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "delimited_paren", move |input| {
        let left = input.offset;
        let (input, _) = char('[')(input)?;
        let (input, ws1) = opt(whitespace)(input)?;
        let (input, inner_items) = opt(token_list)(input)?;
        let (input, ws2) = opt(whitespace)(input)?;
        let (input, _) = char(']')(input)?;
        let right = input.offset;

        let mut items = vec![];

        if let Some(space) = ws1 {
            items.push(space);
        }

        if let Some(inner_items) = inner_items {
            items.extend(inner_items);
        }

        if let Some(space) = ws2 {
            items.push(space);
        }

        Ok((
            input,
            TokenTreeBuilder::tagged_square(items, (left, right, input.extra)),
        ))
    })
}

pub fn delimited_brace(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "delimited_brace", move |input| {
        let left = input.offset;
        let (input, _) = char('{')(input)?;
        let (input, _) = opt(space1)(input)?;
        let (input, items) = opt(token_list)(input)?;
        let (input, _) = opt(space1)(input)?;
        let (input, _) = char('}')(input)?;
        let right = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_brace(
                items.unwrap_or_else(|| vec![]),
                (left, right, input.extra),
            ),
        ))
    })
}

pub fn raw_call(input: NomSpan) -> IResult<NomSpan, Tagged<CallNode>> {
    trace_step(input, "raw_call", move |input| {
        let left = input.offset;
        let (input, items) = token_list(input)?;
        let right = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_call(items, (left, right, input.extra)),
        ))
    })
}

pub fn path(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "path", move |input| {
        let left = input.offset;
        let (input, head) = node1(input)?;
        let (input, _) = tag(".")(input)?;
        let (input, tail) = separated_list(tag("."), alt((member, string)))(input)?;
        let right = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_path((head, tail), (left, right, input.extra)),
        ))
    })
}

pub fn node1(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "node1", alt((leaf, delimited_paren)))
}

pub fn node(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(
        input,
        "node",
        alt((
            path,
            leaf,
            delimited_paren,
            delimited_brace,
            delimited_square,
        )),
    )
}

pub fn pipeline(input: NomSpan) -> IResult<NomSpan, TokenNode> {
    trace_step(input, "pipeline", |input| {
        let start = input.offset;
        let (input, head) = opt(tuple((opt(space1), raw_call, opt(space1))))(input)?;
        let (input, items) = trace_step(
            input,
            "many0",
            many0(tuple((tag("|"), opt(space1), raw_call, opt(space1)))),
        )?;

        let (input, tail) = opt(space1)(input)?;
        let (input, newline) = opt(multispace1)(input)?;

        if input.input_len() != 0 {
            return Err(Err::Error(error_position!(
                input,
                nom::error::ErrorKind::Eof
            )));
        }

        let end = input.offset;

        Ok((
            input,
            TokenTreeBuilder::tagged_pipeline(
                (make_call_list(head, items), tail.map(Tag::from)),
                (start, end, input.extra),
            ),
        ))
    })
}

fn make_call_list(
    head: Option<(Option<NomSpan>, Tagged<CallNode>, Option<NomSpan>)>,
    items: Vec<(NomSpan, Option<NomSpan>, Tagged<CallNode>, Option<NomSpan>)>,
) -> Vec<PipelineElement> {
    let mut out = vec![];

    if let Some(head) = head {
        let el = PipelineElement::new(None, head.0.map(Tag::from), head.1, head.2.map(Tag::from));

        out.push(el);
    }

    for (pipe, ws1, call, ws2) in items {
        let el = PipelineElement::new(
            Some(pipe).map(Tag::from),
            ws1.map(Tag::from),
            call,
            ws2.map(Tag::from),
        );

        out.push(el);
    }

    out
}

fn int<T>(frag: &str, neg: Option<T>) -> i64 {
    let int = FromStr::from_str(frag).unwrap();

    match neg {
        None => int,
        Some(_) => int * -1,
    }
}

fn is_external_word_char(c: char) -> bool {
    match c {
        ';' | '|' | '#' | '-' | '"' | '\'' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '`' => false,
        other if other.is_whitespace() => false,
        _ => true,
    }
}

/// These characters appear in globs and not bare words
fn is_glob_specific_char(c: char) -> bool {
    c == '*' || c == '?'
}

fn is_start_glob_char(c: char) -> bool {
    is_start_bare_char(c) || is_glob_specific_char(c)
}

fn is_glob_char(c: char) -> bool {
    is_bare_char(c) || is_glob_specific_char(c)
}

fn is_start_bare_char(c: char) -> bool {
    match c {
        '+' => false,
        _ if c.is_alphabetic() => true,
        '.' => true,
        '\\' => true,
        '/' => true,
        '_' => true,
        '-' => true,
        '~' => true,
        _ => false,
    }
}

fn is_bare_char(c: char) -> bool {
    match c {
        '+' => false,
        _ if c.is_alphanumeric() => true,
        '.' => true,
        '\\' => true,
        '/' => true,
        '_' => true,
        '-' => true,
        '=' => true,
        '~' => true,
        ':' => true,
        '?' => true,
        _ => false,
    }
}

fn is_id_start(c: char) -> bool {
    unicode_xid::UnicodeXID::is_xid_start(c)
}

fn is_id_continue(c: char) -> bool {
    unicode_xid::UnicodeXID::is_xid_continue(c)
        || match c {
            '-' => true,
            '?' => true,
            '!' => true,
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse::token_tree_builder::TokenTreeBuilder as b;
    use crate::parser::parse::token_tree_builder::{CurriedToken, TokenTreeBuilder};
    use pretty_assertions::assert_eq;

    pub type CurriedNode<T> = Box<dyn FnOnce(&mut TokenTreeBuilder) -> T + 'static>;

    macro_rules! assert_leaf {
        (parsers [ $($name:tt)* ] $input:tt -> $left:tt .. $right:tt { $kind:tt $parens:tt } ) => {
            $(
                assert_eq!(
                    apply($name, stringify!($name), $input),
                    token(RawToken::$kind $parens, $left, $right)
                );
            )*

            assert_eq!(
                apply(leaf, "leaf", $input),
                token(RawToken::$kind $parens, $left, $right)
            );

            assert_eq!(
                apply(leaf, "leaf", $input),
                token(RawToken::$kind $parens, $left, $right)
            );

            assert_eq!(
                apply(node, "node", $input),
                token(RawToken::$kind $parens, $left, $right)
            );
        };

        (parsers [ $($name:tt)* ] $input:tt -> $left:tt .. $right:tt { $kind:tt } ) => {
            $(
                assert_eq!(
                    apply($name, stringify!($name), $input),
                    token(RawToken::$kind, $left, $right)
                );
            )*
        }
    }

    macro_rules! equal_tokens {
        ($source:tt -> $tokens:expr) => {
            let result = apply(pipeline, "pipeline", $source);
            let (expected_tree, expected_source) = TokenTreeBuilder::build(uuid::Uuid::nil(), $tokens);

            if result != expected_tree {
                let debug_result = format!("{}", result.debug($source));
                let debug_expected = format!("{}", expected_tree.debug(&expected_source));

                if debug_result == debug_expected {
                    assert_eq!(
                        result, expected_tree,
                        "NOTE: actual and expected had equivalent debug serializations, source={:?}, debug_expected={:?}",
                        $source,
                        debug_expected
                    )
                } else {
                    assert_eq!(debug_result, debug_expected)
                }
            }

            // apply(pipeline, "pipeline", r#"cargo +nightly run"#),
            // build_token(b::pipeline(vec![(
            //     None,
            //     b::call(
            //         b::bare("cargo"),
            //         vec![
            //             b::sp(),
            //             b::external_word("+nightly"),
            //             b::sp(),
            //             b::bare("run")
            //         ]
            //     ),
            //     None
            // )]))
        };
    }

    #[test]
    fn test_integer() {
        assert_leaf! {
            parsers [ size ]
            "123" -> 0..3 { Number(RawNumber::int((0, 3, test_uuid())).item) }
        }

        assert_leaf! {
            parsers [ size ]
            "-123" -> 0..4 { Number(RawNumber::int((0, 4, test_uuid())).item) }
        }
    }

    #[test]
    fn test_size() {
        assert_leaf! {
            parsers [ size ]
            "123MB" -> 0..5 { Size(RawNumber::int((0, 3, test_uuid())).item, Unit::MB) }
        }

        assert_leaf! {
            parsers [ size ]
            "10GB" -> 0..4 { Size(RawNumber::int((0, 2, test_uuid())).item, Unit::GB) }
        }
    }

    #[test]
    fn test_operator() {
        assert_eq!(apply(node, "node", ">"), build_token(b::op(">")));

        // assert_leaf! {
        //     parsers [ operator ]
        //     ">=" -> 0..2 { Operator(Operator::GreaterThanOrEqual) }
        // }

        // assert_leaf! {
        //     parsers [ operator ]
        //     "<" -> 0..1 { Operator(Operator::LessThan) }
        // }

        // assert_leaf! {
        //     parsers [ operator ]
        //     "<=" -> 0..2 { Operator(Operator::LessThanOrEqual) }
        // }

        // assert_leaf! {
        //     parsers [ operator ]
        //     "==" -> 0..2 { Operator(Operator::Equal) }
        // }

        // assert_leaf! {
        //     parsers [ operator ]
        //     "!=" -> 0..2 { Operator(Operator::NotEqual) }
        // }
    }

    #[test]
    fn test_string() {
        assert_leaf! {
            parsers [ string dq_string ]
            r#""hello world""# -> 0..13 { String(tag(1, 12)) }
        }

        assert_leaf! {
            parsers [ string sq_string ]
            r"'hello world'" -> 0..13 { String(tag(1, 12)) }
        }
    }

    #[test]
    fn test_bare() {
        assert_leaf! {
            parsers [ bare ]
            "hello" -> 0..5 { Bare }
        }

        assert_leaf! {
            parsers [ bare ]
            "chrome.exe" -> 0..10 { Bare }
        }

        assert_leaf! {
            parsers [ bare ]
            r"C:\windows\system.dll" -> 0..21 { Bare }
        }

        assert_leaf! {
            parsers [ bare ]
            r"C:\Code\-testing\my_tests.js" -> 0..28 { Bare }
        }
    }

    #[test]
    fn test_flag() {
        // assert_leaf! {
        //     parsers [ flag ]
        //     "--hello" -> 0..7 { Flag(Tagged::from_item(FlagKind::Longhand, span(2, 7))) }
        // }

        // assert_leaf! {
        //     parsers [ flag ]
        //     "--hello-world" -> 0..13 { Flag(Tagged::from_item(FlagKind::Longhand, span(2, 13))) }
        // }
    }

    #[test]
    fn test_shorthand() {
        // assert_leaf! {
        //     parsers [ shorthand ]
        //     "-alt" -> 0..4 { Flag(Tagged::from_item(FlagKind::Shorthand, span(1, 4))) }
        // }
    }

    #[test]
    fn test_variable() {
        assert_leaf! {
            parsers [ var ]
            "$it" -> 0..3 { Variable(tag(1, 3)) }
        }

        assert_leaf! {
            parsers [ var ]
            "$name" -> 0..5 { Variable(tag(1, 5)) }
        }
    }

    #[test]
    fn test_external() {
        assert_leaf! {
            parsers [ external ]
            "^ls" -> 0..3 { ExternalCommand(tag(1, 3)) }
        }
    }

    #[test]
    fn test_delimited_paren() {
        assert_eq!(
            apply(node, "node", "(abc)"),
            build_token(b::parens(vec![b::bare("abc")]))
        );

        assert_eq!(
            apply(node, "node", "(  abc  )"),
            build_token(b::parens(vec![b::ws("  "), b::bare("abc"), b::ws("  ")]))
        );

        assert_eq!(
            apply(node, "node", "(  abc def )"),
            build_token(b::parens(vec![
                b::ws("  "),
                b::bare("abc"),
                b::sp(),
                b::bare("def"),
                b::sp()
            ]))
        );

        assert_eq!(
            apply(node, "node", "(  abc def 123 456GB )"),
            build_token(b::parens(vec![
                b::ws("  "),
                b::bare("abc"),
                b::sp(),
                b::bare("def"),
                b::sp(),
                b::int(123),
                b::sp(),
                b::size(456, "GB"),
                b::sp()
            ]))
        );
    }

    #[test]
    fn test_delimited_square() {
        assert_eq!(
            apply(node, "node", "[abc]"),
            build_token(b::square(vec![b::bare("abc")]))
        );

        assert_eq!(
            apply(node, "node", "[  abc  ]"),
            build_token(b::square(vec![b::ws("  "), b::bare("abc"), b::ws("  ")]))
        );

        assert_eq!(
            apply(node, "node", "[  abc def ]"),
            build_token(b::square(vec![
                b::ws("  "),
                b::bare("abc"),
                b::sp(),
                b::bare("def"),
                b::sp()
            ]))
        );

        assert_eq!(
            apply(node, "node", "[  abc def 123 456GB ]"),
            build_token(b::square(vec![
                b::ws("  "),
                b::bare("abc"),
                b::sp(),
                b::bare("def"),
                b::sp(),
                b::int(123),
                b::sp(),
                b::size(456, "GB"),
                b::sp()
            ]))
        );
    }

    #[test]
    fn test_path() {
        let _ = pretty_env_logger::try_init();
        assert_eq!(
            apply(node, "node", "$it.print"),
            build_token(b::path(b::var("it"), vec![b::member("print")]))
        );

        assert_eq!(
            apply(node, "node", "$head.part1.part2"),
            build_token(b::path(
                b::var("head"),
                vec![b::member("part1"), b::member("part2")]
            ))
        );

        assert_eq!(
            apply(node, "node", "( hello ).world"),
            build_token(b::path(
                b::parens(vec![b::sp(), b::bare("hello"), b::sp()]),
                vec![b::member("world")]
            ))
        );

        assert_eq!(
            apply(node, "node", "( hello ).\"world\""),
            build_token(b::path(
                b::parens(vec![b::sp(), b::bare("hello"), b::sp()],),
                vec![b::string("world")]
            ))
        );
    }

    #[test]
    fn test_nested_path() {
        assert_eq!(
            apply(
                node,
                "node",
                "( $it.is.\"great news\".right yep $yep ).\"world\""
            ),
            build_token(b::path(
                b::parens(vec![
                    b::sp(),
                    b::path(
                        b::var("it"),
                        vec![b::member("is"), b::string("great news"), b::member("right")]
                    ),
                    b::sp(),
                    b::bare("yep"),
                    b::sp(),
                    b::var("yep"),
                    b::sp()
                ]),
                vec![b::string("world")]
            ))
        )
    }

    #[test]
    fn test_smoke_single_command() {
        assert_eq!(
            apply(raw_call, "raw_call", "git add ."),
            build(b::call(
                b::bare("git"),
                vec![b::sp(), b::bare("add"), b::sp(), b::bare(".")]
            ))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "open Cargo.toml"),
            build(b::call(
                b::bare("open"),
                vec![b::sp(), b::bare("Cargo.toml")]
            ))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "select package.version"),
            build(b::call(
                b::bare("select"),
                vec![b::sp(), b::bare("package.version")]
            ))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "echo $it"),
            build(b::call(b::bare("echo"), vec![b::sp(), b::var("it")]))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "open Cargo.toml --raw"),
            build(b::call(
                b::bare("open"),
                vec![b::sp(), b::bare("Cargo.toml"), b::sp(), b::flag("raw")]
            ))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "open Cargo.toml -r"),
            build(b::call(
                b::bare("open"),
                vec![b::sp(), b::bare("Cargo.toml"), b::sp(), b::shorthand("r")]
            ))
        );

        assert_eq!(
            apply(raw_call, "raw_call", "config --set tabs 2"),
            build(b::call(
                b::bare("config"),
                vec![
                    b::sp(),
                    b::flag("set"),
                    b::sp(),
                    b::bare("tabs"),
                    b::sp(),
                    b::int(2)
                ]
            ))
        );
    }

    #[test]
    fn test_external_word() {
        let _ = pretty_env_logger::try_init();

        equal_tokens!(
            "cargo +nightly run" ->
            b::pipeline(vec![(
                None,
                b::call(
                    b::bare("cargo"),
                    vec![
                        b::sp(),
                        b::external_word("+nightly"),
                        b::sp(),
                        b::bare("run")
                    ]
                ),
                None
            )])
        );

        equal_tokens!(
            "rm foo%bar" ->
            b::pipeline(vec![(
                None,
                b::call(b::bare("rm"), vec![b::sp(), b::external_word("foo%bar"),]),
                None
            )])
        );

        equal_tokens!(
            "rm foo%bar" ->
            b::pipeline(vec![(
                None,
                b::call(b::bare("rm"), vec![b::sp(), b::external_word("foo%bar"),]),
                None
            )])
        );
    }

    #[test]
    fn test_smoke_pipeline() {
        let _ = pretty_env_logger::try_init();

        assert_eq!(
            apply(
                pipeline,
                "pipeline",
                r#"git branch --merged | split-row "`n" | where $it != "* master""#
            ),
            build_token(b::pipeline(vec![
                (
                    None,
                    b::call(
                        b::bare("git"),
                        vec![b::sp(), b::bare("branch"), b::sp(), b::flag("merged")]
                    ),
                    Some(" ")
                ),
                (
                    Some(" "),
                    b::call(b::bare("split-row"), vec![b::sp(), b::string("`n")]),
                    Some(" ")
                ),
                (
                    Some(" "),
                    b::call(
                        b::bare("where"),
                        vec![
                            b::sp(),
                            b::var("it"),
                            b::sp(),
                            b::op("!="),
                            b::sp(),
                            b::string("* master")
                        ]
                    ),
                    None
                )
            ]))
        );

        assert_eq!(
            apply(pipeline, "pipeline", "ls | where { $it.size > 100 }"),
            build_token(b::pipeline(vec![
                (None, b::call(b::bare("ls"), vec![]), Some(" ")),
                (
                    Some(" "),
                    b::call(
                        b::bare("where"),
                        vec![
                            b::sp(),
                            b::braced(vec![
                                b::path(b::var("it"), vec![b::member("size")]),
                                b::sp(),
                                b::op(">"),
                                b::sp(),
                                b::int(100)
                            ])
                        ]
                    ),
                    None
                )
            ]))
        )
    }

    fn apply<T>(
        f: impl Fn(NomSpan) -> Result<(NomSpan, T), nom::Err<(NomSpan, nom::error::ErrorKind)>>,
        desc: &str,
        string: &str,
    ) -> T {
        match f(NomSpan::new_extra(string, uuid::Uuid::nil())) {
            Ok(v) => v.1,
            Err(other) => {
                println!("{:?}", other);
                println!("for {} @ {}", string, desc);
                panic!("No dice");
            }
        }
    }

    fn tag(left: usize, right: usize) -> Tag {
        Tag::from((left, right, uuid::Uuid::nil()))
    }

    fn delimited(
        delimiter: Tagged<Delimiter>,
        children: Vec<TokenNode>,
        left: usize,
        right: usize,
    ) -> TokenNode {
        let node = DelimitedNode::new(*delimiter, children);
        let spanned = node.tagged((left, right, delimiter.tag.origin));
        TokenNode::Delimited(spanned)
    }

    fn path(head: TokenNode, tail: Vec<Token>, left: usize, right: usize) -> TokenNode {
        let tag = head.tag();

        let node = PathNode::new(
            Box::new(head),
            tail.into_iter().map(TokenNode::Token).collect(),
        );
        let spanned = node.tagged((left, right, tag.origin));
        TokenNode::Path(spanned)
    }

    fn token(token: RawToken, left: usize, right: usize) -> TokenNode {
        TokenNode::Token(token.tagged((left, right, uuid::Uuid::nil())))
    }

    fn build<T>(block: CurriedNode<T>) -> T {
        let mut builder = TokenTreeBuilder::new(uuid::Uuid::nil());
        block(&mut builder)
    }

    fn build_token(block: CurriedToken) -> TokenNode {
        TokenTreeBuilder::build(uuid::Uuid::nil(), block).0
    }

    fn test_uuid() -> uuid::Uuid {
        uuid::Uuid::nil()
    }
}
