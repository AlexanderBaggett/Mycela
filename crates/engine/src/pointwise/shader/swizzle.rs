//! Lower swizzle assignments that the pinned Naga frontend cannot express.
use super::tokens;
use naga::front::wgsl::ParseError;
use std::{fmt::Write, ops::Range};

const NOT_REFERENCE: &str = "the operand of the `&` operator must be a reference";
const NOT_POINTER: &str = "the operand of the `*` operator must be a pointer";
const INVALID_DESTINATION: &str = "invalid left-hand side of assignment";
const INVALID_INCR_DECR: &str =
    "increment/decrement operation requires reference type to be one of i32 or u32";
const COMPONENTS: &[u8] = b"xyzw";

#[derive(Default)]
pub(super) struct SwizzleLowering {
    assignments: Vec<Assignment>,
}

impl SwizzleLowering {
    pub(super) fn rewrite(
        &mut self,
        source: &mut String,
        error: &ParseError,
        enabled: bool,
        reserved: bool,
    ) -> Result<bool, String> {
        let message = error.message();
        let mut labels = error.labels();
        let Some(span) = labels.next().and_then(|(span, _)| span.to_range()) else {
            return Ok(false);
        };
        if labels.next().is_some() {
            return Ok(false);
        }
        if matches!(message, NOT_REFERENCE | NOT_POINTER) {
            return self.resolve_address(source, message, &span);
        }
        if !matches!(message, INVALID_DESTINATION | INVALID_INCR_DECR) {
            return Ok(false);
        }
        let Some((range, mut assignment)) = Assignment::from_source(source, span.clone()) else {
            return Ok(false);
        };
        if !enabled {
            return Err(format!(
                "Add `requires swizzle_assignment;` to use this assignment.\n{}",
                error.emit_to_string(source)
            ));
        }
        if reserved {
            return Err("Names starting with mycela_swizzle_ are reserved.".into());
        }
        if let Some((range, loop_source)) = lower_for_header(source, span.start) {
            source.replace_range(range, &loop_source);
        } else {
            assignment.render(self.assignments.len())?;
            source.replace_range(range, &assignment.generated);
            self.assignments.push(assignment);
        }
        Ok(true)
    }

    fn resolve_address(
        &mut self,
        source: &mut String,
        message: &str,
        span: &Range<usize>,
    ) -> Result<bool, String> {
        for (id, assignment) in self.assignments.iter_mut().enumerate() {
            let Some(start) = source.find(&assignment.generated) else {
                continue;
            };
            let relative_span = span.start.saturating_sub(start)..span.end.saturating_sub(start);
            if span.start < start || !assignment.address_failure(message, &relative_span) {
                continue;
            }
            let end = start + assignment.generated.len();
            if assignment.take_address {
                // A swizzle can start from a reference or a pointer value.
                assignment.take_address = false;
            } else if !assignment.compose_previous_member() {
                return Err(format!(
                    "Swizzle destination `{}` needs a writable vector.",
                    assignment.original
                ));
            }
            assignment.render(id)?;
            source.replace_range(start..end, &assignment.generated);
            return Ok(true);
        }
        Ok(false)
    }
}

enum Destination {
    Components(Vec<usize>),
    Indexed { index: String, map: Vec<usize> },
}

struct Assignment {
    original: String,
    base: String,
    destination: Destination,
    update: String,
    take_address: bool,
    generated: String,
    address_span: Range<usize>,
}

impl Assignment {
    fn from_source(source: &str, span: Range<usize>) -> Option<(Range<usize>, Self)> {
        let target = unwrap_parentheses(&source[span.clone()]);
        let (base, destination) = if let Some((base, components)) = member(target) {
            if components.len() == 1 && member(unwrap_parentheses(base))?.1.len() < 2 {
                return None;
            }
            (base, Destination::Components(components))
        } else {
            let (base, index) = index_expression(target)?;
            if member(unwrap_parentheses(base))?.1.len() < 2 {
                return None;
            }
            (
                base,
                Destination::Indexed {
                    index: index.to_owned(),
                    map: Vec::new(),
                },
            )
        };
        let after = &source[span.end..];
        let tail = tokens(after);
        let first = tail.first()?;
        let operator = [
            "<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "=", "++", "--",
        ]
        .into_iter()
        .find(|operator| after[first.start..].starts_with(operator))?;
        // A continuing clause ends at the enclosing header's closing parenthesis.
        // Other assignment expressions end at the semicolon.
        let mut depth = 0;
        let end = tail.iter().find(|part| match &after[(*part).clone()] {
            "(" => {
                depth += 1;
                false
            }
            ")" if depth > 0 => {
                depth -= 1;
                false
            }
            ")" | ";" if depth == 0 => true,
            _ => false,
        })?;
        let rhs = after.get(first.start + operator.len()..end.start)?.trim();
        let postfix = matches!(operator, "++" | "--");
        let multi = matches!(&destination, Destination::Components(lanes) if lanes.len() > 1);
        if postfix && multi || !postfix && rhs.is_empty() {
            return None;
        }
        let update = if postfix {
            operator.to_owned()
        } else {
            format!("{operator} {rhs}")
        };
        Some((
            span.start..span.end + end.end,
            Self {
                original: target.to_owned(),
                base: base.to_owned(),
                destination,
                update,
                take_address: true,
                generated: String::new(),
                address_span: 0..0,
            },
        ))
    }

    fn address_failure(&self, message: &str, span: &Range<usize>) -> bool {
        (message == NOT_REFERENCE && self.take_address
            || message == NOT_POINTER && !self.take_address)
            && span.start >= self.address_span.start
            && span.end <= self.address_span.end
    }

    fn compose_previous_member(&mut self) -> bool {
        let Some((base, components)) = member(unwrap_parentheses(&self.base)) else {
            return false;
        };
        if components.len() < 2 {
            return false;
        }
        let lanes = match &mut self.destination {
            Destination::Components(lanes) => lanes,
            Destination::Indexed { map, .. } => map,
        };
        if lanes.iter().any(|&lane| lane >= components.len()) {
            return false;
        }
        if lanes.is_empty() {
            *lanes = components;
        } else {
            for lane in lanes {
                *lane = components[*lane];
            }
        }
        self.base = base.to_owned();
        self.take_address = true;
        true
    }

    fn render(&mut self, id: usize) -> Result<(), String> {
        let tag = format!("mycela_swizzle_base_{id}");
        let mut output = format!("{{ let {tag} = ");
        let base_start = output.len();
        if self.take_address {
            output.push('&');
        }
        write!(output, "({}); ", self.base).unwrap();
        let base_end = output.len();
        let expression_start = output.len();
        write!(output, "let mycela_swizzle_pointer = &(*{tag}); ").unwrap();
        self.address_span = if self.take_address {
            base_start..base_end
        } else {
            expression_start..output.len()
        };

        let update = &self.update;
        let vector = "(*mycela_swizzle_pointer)";
        match &self.destination {
            Destination::Components(lanes) if lanes.len() > 1 => {
                if lanes
                    .iter()
                    .enumerate()
                    .any(|(index, lane)| lanes[..index].contains(lane))
                {
                    return Err(
                        "Swizzle assignment requires distinct destination components.".into(),
                    );
                }
                let components: String = lanes
                    .iter()
                    .map(|&lane| char::from(COMPONENTS[lane]))
                    .collect();
                // The typed temporary gives abstract numbers the assignment's type.
                // Compound assignments take the selected values before evaluating the RHS.
                write!(
                    output,
                    "var mycela_swizzle_update = {vector}.{components}; \
                     mycela_swizzle_update {update}; \
                     var mycela_swizzle_vector = {vector};"
                )
                .unwrap();
                for (dst, src) in components.chars().zip("xyzw".chars()) {
                    write!(
                        output,
                        " mycela_swizzle_vector.{dst} = mycela_swizzle_update.{src};"
                    )
                    .unwrap();
                }
                // The RHS can change the original vector, including unselected lanes.
                write!(output, " {vector} = mycela_swizzle_vector;").unwrap();
            }
            Destination::Components(lanes) => {
                write!(
                    output,
                    "{vector}.{} {update};",
                    char::from(COMPONENTS[lanes[0]])
                )
                .unwrap();
            }
            Destination::Indexed { index, map } if map.is_empty() => {
                write!(output, "{vector}[{index}] {update};").unwrap();
            }
            Destination::Indexed { index, map } => {
                let mapping = map
                    .iter()
                    .map(|lane| format!("{lane}u"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(output, "{vector}[array({mapping})[{index}]] {update};").unwrap();
            }
        }
        output.push_str(" }");
        self.generated = output;
        Ok(())
    }
}

fn member(source: &str) -> Option<(&str, Vec<usize>)> {
    let parts = tokens(source);
    let [.., dot, field] = parts.as_slice() else {
        return None;
    };
    let name = &source[field.clone()];
    if &source[dot.clone()] != "." || !(1..=4).contains(&name.len()) {
        return None;
    }
    let components = [b"xyzw", b"rgba"].into_iter().find_map(|alphabet| {
        name.bytes()
            .map(|letter| alphabet.iter().position(|&candidate| candidate == letter))
            .collect()
    })?;
    Some((source[..dot.start].trim(), components))
}

fn index_expression(source: &str) -> Option<(&str, &str)> {
    let parts = tokens(source);
    let (last, before) = parts.split_last()?;
    if &source[last.clone()] != "]" {
        return None;
    }
    let mut depth = 1;
    let opening = before.iter().rev().find(|part| {
        match &source[(*part).clone()] {
            "]" => depth += 1,
            "[" => depth -= 1,
            _ => {}
        }
        depth == 0
    })?;
    Some((
        source[..opening.start].trim(),
        source[opening.end..last.start].trim(),
    ))
}

fn unwrap_parentheses(mut source: &str) -> &str {
    loop {
        let parts = tokens(source);
        let [first, inside @ .., last] = parts.as_slice() else {
            return source;
        };
        if &source[first.clone()] != "(" || &source[last.clone()] != ")" {
            return source;
        }
        let mut depth = 1;
        for part in inside {
            match &source[part.clone()] {
                "(" => depth += 1,
                ")" => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return source;
            }
        }
        source = &source[first.end..last.start];
    }
}

/// Put the for initializer and continuing clause in statement positions, where
/// the next lowering pass can use a compound statement for a swizzle write.
fn lower_for_header(source: &str, lhs: usize) -> Option<(Range<usize>, String)> {
    let parts = tokens(source);
    let before = parts.partition_point(|part| part.start < lhs);
    let mut parentheses = Vec::new();
    for (index, part) in parts[..before].iter().enumerate() {
        match &source[part.clone()] {
            "(" => parentheses.push(index),
            ")" => {
                parentheses.pop();
            }
            _ => {}
        }
    }
    let open = *parentheses.last()?;
    let keyword = parts.get(open.checked_sub(1)?)?;
    if &source[keyword.clone()] != "for" {
        return None;
    }
    let mut depth = 1;
    let mut separators = Vec::new();
    let mut close = None;
    for (index, part) in parts.iter().enumerate().skip(open + 1) {
        match &source[part.clone()] {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    close = Some(index);
                    break;
                }
            }
            ";" if depth == 1 => separators.push(part),
            _ => {}
        }
    }
    let close = close?;
    let [init_end, test_end] = separators.as_slice() else {
        return None;
    };
    let body_start = parts.get(close + 1)?;
    if &source[body_start.clone()] != "{" {
        return None;
    }
    let mut depth = 1;
    let body_end = parts[close + 2..].iter().find(|part| {
        match &source[(*part).clone()] {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
        depth == 0
    })?;
    let init = &source[parts[open].end..init_end.end];
    let test = &source[init_end.end..test_end.start];
    let step = &source[test_end.end..parts[close].start];
    let body = &source[body_start.start..body_end.end];
    let guard = if tokens(test).is_empty() {
        String::new()
    } else {
        format!("if !({test}) {{ break; }}")
    };
    // Keep the body scoped separately from continuing, including any shadowed names.
    let replacement = format!("{{ {init} loop {{ {guard} {body} continuing {{ {step}; }} }} }}");
    Some((keyword.start..body_end.end, replacement))
}
