/// It should be noted that indentation checks do not verify that mixed
/// spaces and tabs do not depend on the size of a tab stop for correctness.
///
/// Normalize error messages
///
use std::str::Chars;
use std::iter::Peekable;
use iter::DoublePeekable;
use tokens::{Token, keyword_lookup};

const TAB_STOP_SIZE: u32 = 8;

pub type ResultToken = Result<Token, String>;

pub struct Lexer<'a>
{
   lexer: Peekable<InternalLexer<'a>>
}

impl <'a> Lexer<'a>
{
   pub fn new<'b, I>(lines: I)
      -> Lexer<'b>
      where I: Iterator<Item=&'b str> + 'b
   {
      Lexer{lexer: InternalLexer::new(lines).peekable()}
   }

   fn string_follows(&mut self)
      -> Option<String>
   {
      match self.lexer.peek()
      {
         Some(&(_, Ok(Token::String(_)))) =>
         {
            Some(self.lexer.next().unwrap().1.ok().unwrap().lexeme())
         },
         _ => None,
      }
   }
}

impl <'a> Iterator for Lexer<'a>
{
   type Item = (usize, ResultToken);

   fn next(&mut self)
      -> Option<Self::Item>
   {
      match self.lexer.next()
      {
         Some((line_number, Ok(Token::String(s)))) =>
         {
            let mut token_str = s.clone();
            while let Some(follow) = self.string_follows()
            {
               token_str.push_str(&follow)
            }
            Some((line_number, Ok(Token::String(token_str))))
         },
         result => result,
      }
   }
}

pub struct InternalLexer<'a>
{
   indent_stack: Vec<u32>,
   dedent_count: i32,
   lines: Box<Iterator<Item=Line<'a>> + 'a>,
   current_line: Option<Line<'a>>,
}

impl <'a> Iterator for InternalLexer<'a>
{
   type Item = (usize, ResultToken);

   fn next(&mut self)
      -> Option<Self::Item>
   {
      self.next_token()
   }
}

struct Line<'a>
{
   number: usize,
   indentation: u32,
   leading_spaces: String,
   chars: DoublePeekable<Chars<'a>>
}

impl <'a> Line<'a>
{
   fn new<'b>(number: usize, indentation: u32, leading_spaces: String,
      chars: DoublePeekable<Chars<'b>>)
      -> Line<'b>
   {
      Line {number: number, indentation: indentation,
         leading_spaces: leading_spaces, chars: chars}
   }
}

impl <'a> InternalLexer<'a>
{
   pub fn new<'b, I>(lines: I)
      -> InternalLexer<'b>
      where I: Iterator<Item=&'b str> + 'b
   {
      let iter = (1..).zip(lines)
         .map(|(n, line)| (n, DoublePeekable::new(line.chars())))
         .map(|(n, mut chars)|
            {
               let (indentation, spaces) = count_indentation(&mut chars);
               Line::new(n, indentation, spaces, chars)
            });
      ;
      InternalLexer{indent_stack: vec![0],
         dedent_count: 0,
         lines: Box::new(iter),
         current_line: None,
      }
   }

   fn next_token(&mut self)
      -> Option<(usize, ResultToken)>
   {
      let current_line = self.current_line.take();
      let result = self.next_token_line(current_line);
      self.current_line = result.1;
      result.0
   }

   fn next_token_line(&mut self, current_line: Option<Line<'a>>)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      if let Some(mut current_line) = current_line
      {
         if self.dedent_count != 0
         {
            self.process_dedents(current_line)
         }
         else
         {
            consume_space_to_next(&mut current_line);
            match current_line.chars.peek()
            {
               Some(&'#') => process_newline(current_line),
               Some(&c) if is_xid_start(c) =>
                  process_identifier(current_line),
               Some(&c) if c.is_digit(10) => process_number(current_line),
               Some(&'.') =>
               {
                  match current_line.chars.peek_second()
                  {
                     Some(&c) if c.is_digit(10) => 
                        process_number(current_line),
                     _ => process_symbols(current_line),
                  }
               }
               Some(&'\\') => self.process_line_join(current_line),
               Some(&'\'') | Some(&'"') => self.process_string(current_line),
               Some(_) => process_symbols(current_line),
               None => process_newline(current_line),
            }
         }
      }
      else
      {
         match self.lines.next()
         {
            None if self.indent_stack.len() <= 1 => (None, None),
            None => (Some((0, Ok(Token::Dedent))), None),
            Some(newline) => self.process_line_start(newline)
         }
      }
   }

   fn process_line_join(&mut self, mut current_line: Line<'a>)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      current_line.chars.next();
      if current_line.chars.peek().is_none()
      {
         // explicit line join
         let newline = self.lines.next();
         self.next_token_line(newline)
      }
      else
      {
         let line_number = current_line.number;
         (Some((line_number, Err("** bad \\ **".to_string()))),
            Some(current_line))
      }
   }

   fn process_string(&mut self, mut line: Line<'a>)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      let quote = line.chars.next().unwrap();
      if line.chars.peek().map_or(false, |&c| c == quote) &&
         line.chars.peek_second().map_or(false, |&c| c == quote)
      {
         self.process_long_string(line, quote)
      }
      else
      {
         self.process_short_string(line, quote)
      }
   }

   fn process_short_string(&mut self, line: Line<'a>, quote: char)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      let first_line_number = line.number;
      let mut current_line = line;
      let mut token_str = String::new();

      while current_line.chars.peek().map_or(false, |&c| c != quote)
      {
         match current_line.chars.next()
         {
            Some('\\') =>
            {
               let line_number = current_line.number;
               let (new_line, new_token_str) =
                  self.handle_escaped_character(current_line, token_str);

               token_str = new_token_str;

               if new_line.is_some()
               {
                  current_line = new_line.unwrap();
               }
               else
               {
                  return (Some((line_number + 1,
                     Err(format!("unterminated {}", quote).to_string()))),
                     new_line)
               }
            },
            Some(cur_char) => token_str.push(cur_char),
            _ => unreachable!(),
         }
      }

      if current_line.chars.peek().is_none()
      {
         (Some((current_line.number,
            Err(format!("unterminated {}", quote).to_string()))),
            Some(current_line))
      }
      else
      {
         current_line.chars.next().unwrap();    // consume quote
         (Some((first_line_number, Ok(Token::String(token_str)))),
            Some(current_line))
      }
   }

   fn process_long_string(&mut self, mut line: Line<'a>, quote: char)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      // consume second two quote characters
      line.chars.next();
      line.chars.next();

      let first_line_number = line.number;
      let mut current_line = line;
      let mut token_str = String::new();

      loop
      {
         match current_line.chars.next()
         {
            Some('\\') =>
            {
               let line_number = current_line.number;
               let (new_line, new_token_str) =
                  self.handle_escaped_character(current_line, token_str);

               token_str = new_token_str;

               if new_line.is_some()
               {
                  current_line = new_line.unwrap();
               }
               else
               {
                  return (Some((line_number + 1,
                     Err(format!("unterminated {}", quote).to_string()))),
                     None)
               }
            },
            Some(cur_char) if cur_char == quote =>
            {
               if current_line.chars.peek().map_or(false, |&c| c == quote) &&
                  current_line.chars.peek_second()
                     .map_or(false, |&c| c == quote)
               {
                  break;
               }
               else
               {
                  token_str.push(cur_char);
               }
            },
            Some(cur_char) => token_str.push(cur_char),
            None => // end of line
            {
               token_str.push('\n');
               let line_number = current_line.number;
               match self.lines.next()
               {
                  Some(line) =>
                  {
                     token_str.push_str(&line.leading_spaces);
                     current_line = line;
                  },
                  _ => return (Some((line_number + 1,
                     Err(format!("unterminated {0}{0}{0}", quote)
                        .to_string()))), None)
               }
            },
         }
      }

      (Some((first_line_number, Ok(Token::String(token_str)))),
         Some(current_line))
   }

   fn handle_escaped_character(&mut self, mut line: Line<'a>,
      mut token_str: String)
      -> (Option<Line<'a>>, String)
   {
      match line.chars.next()
      {
         None => // end of line escape, join lines
         {
            let next_line = self.lines.next();
            if next_line.is_some()
            {
               token_str.push_str(&next_line.as_ref().unwrap().leading_spaces);
            }
            (next_line, token_str)
         },
         Some('\\') =>
         {
            token_str.push('\\');
            (Some(line), token_str)
         },
         Some('\'') =>
         {
            token_str.push('\'');
            (Some(line), token_str)
         },
         Some('"') =>
         {
            token_str.push('"');
            (Some(line), token_str)
         },
         Some('a') =>
         {
            token_str.push('\x07'); // BEL
            (Some(line), token_str)
         },
         Some('b') =>
         {
            token_str.push('\x08'); // BS
            (Some(line), token_str)
         },
         Some('f') =>
         {
            token_str.push('\x0C'); // FF
            (Some(line), token_str)
         },
         Some('n') =>
         {
            token_str.push('\n');
            (Some(line), token_str)
         },
         Some('r') =>
         {
            token_str.push('\r');
            (Some(line), token_str)
         },
         Some('t') =>
         {
            token_str.push('\t');
            (Some(line), token_str)
         },
         Some('v') =>
         {
            token_str.push('\x0B'); // VT
            (Some(line), token_str)
         },
         Some(c) if c.is_digit(8) =>
         {
            unimplemented!();
         },
         Some('x') =>
         {
            unimplemented!();
         },
         Some('N') =>
         {
            unimplemented!();
         },
         Some('u') =>
         {
            unimplemented!();
         },
         Some('U') =>
         {
            unimplemented!();
         },
         Some(c) =>
         {
            token_str.push('\\');
            token_str.push(c);
            (Some(line), token_str)
         },
      }
   }

   fn process_line_start(&mut self, newline: Line<'a>)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      if let Some(&previous_indent) = self.indent_stack.last()
      {
         if newline.chars.peek().is_none()
         {
            self.next_token_line(None)
         }
         else if newline.indentation > previous_indent
         {
            self.indent_stack.push(newline.indentation);
            (Some((newline.number, Ok(Token::Indent))), Some(newline))
         }
         else if newline.indentation < previous_indent
         {
            let stack_len = self.indent_stack.len();
            let mut i = stack_len - 1;
            while newline.indentation < self.indent_stack[i]
            {
               i -= 1;
            }
            self.indent_stack.truncate(i + 1);
            self.dedent_count = (stack_len - 1 - i) as i32;
            if self.indent_stack[i] != newline.indentation
            {
               self.dedent_count *= -1;
            }
            self.next_token_line(Some(newline))
         }
         else
         {
            self.next_token_line(Some(newline))
         }
      }
      else
      {
         panic!("Internal indentation stack error!")
      }
   }

   fn process_dedents(&mut self, current_line: Line<'a>)
      -> (Option<(usize, ResultToken)>, Option<Line<'a>>)
   {
      if self.dedent_count == -1
      {
         self.dedent_count = 0;
         (Some((current_line.number, Err("** DEDENT ERROR **".to_string()))),
            Some(current_line))
      }
      else
      {
         self.dedent_count += if self.dedent_count < 0 {1} else {-1};
         (Some((current_line.number, Ok(Token::Dedent))), Some(current_line))
      }
   }
}

fn process_identifier(mut current_line: Line)
   -> (Option<(usize, ResultToken)>, Option<Line>)
{
   let result = build_identifier(&mut current_line);
   (Some(result), Some(current_line))
}

fn build_identifier(line: &mut Line)
   -> (usize, ResultToken)
{
   let token = keyword_lookup(
      consume_and_while(String::new(), line, |c| is_xid_continue(c)));
   (line.number, Ok(token))
}

fn process_number(mut current_line: Line)
   -> (Option<(usize, ResultToken)>, Option<Line>)
{
   let result = build_number(&mut current_line);
   (Some(result), Some(current_line))
}

fn build_number(line: &mut Line)
   -> (usize, ResultToken)
{
   match line.chars.peek()
   {
      Some(&'0') =>
      {
         let result = build_zero_prefixed_number(line);
         (line.number, result)
      },
      Some(&'.') =>
      {
         let result = build_dot_prefixed(line);
         (line.number, result)
      },
      _ =>
      {
         let result = build_decimal_number(line);
         let result = build_float_part(result, line);

         (line.number, result)
      },
   }
}

fn build_dot_prefixed(line: &mut Line)
   -> ResultToken
{
   let mut token_str = String::new();
   token_str.push(line.chars.next().unwrap());

   match line.chars.peek()
   {
      Some(&c) if c.is_digit(10) =>
      {
         let result = require_radix_digits(token_str, line, 10,
            |s| Token::Float(s));
         let result = build_exp_float(result, line);
         let result = build_img_float(result, line);
         result
      },
      _ => Err("internal error: dot".to_string())
   }
}

fn build_decimal_number(line: &mut Line)
   -> ResultToken
{
   require_radix_digits(String::new(), line, 10, |s| Token::DecInteger(s))
}

fn build_zero_prefixed_number(line: &mut Line)
   -> ResultToken
{
   let mut token_str = String::new();

   token_str.push(line.chars.next().unwrap());

   match line.chars.peek()
   {
      Some(&'o') | Some(&'O') =>
      {
         token_str.push(line.chars.next().unwrap());
         require_radix_digits(token_str, line, 8, |s| Token::OctInteger(s))
      },
      Some(&'x') | Some(&'X') =>
      {
         token_str.push(line.chars.next().unwrap());
         require_radix_digits(token_str, line, 16, |s| Token::HexInteger(s))
      },
      Some(&'b') | Some(&'B') =>
      {
         token_str.push(line.chars.next().unwrap());
         require_radix_digits(token_str, line, 2, |s| Token::BinInteger(s))
      },
      Some(&'0') => 
      {
         token_str = consume_and_while(token_str, line,
            |c| c.is_digit(1));
         if line.chars.peek().map_or(false, |c| c.is_digit(10))
         {
            let token = require_radix_digits(token_str, line, 10,
               |s| Token::DecInteger(s));
            require_float_part(token, line)
         }
         else
         {
            build_float_part(Ok(Token::DecInteger(token_str)), line)
         }
      },
      Some(&c) if c.is_digit(10) =>
      {
         let token = require_radix_digits(token_str, line, 10,
               |s| Token::DecInteger(s));
         require_float_part(token, line)
      },
      _ => build_float_part(Ok(Token::DecInteger(token_str)), line),
   }
}

fn require_radix_digits<F>(token_str: String, line: &mut Line,
   radix: u32, token_type: F)
   -> ResultToken
      where F: Fn(String) -> Token
{
   match line.chars.peek()
   {
      Some(&c) if c.is_digit(radix) =>
         Ok(token_type(consume_and_while(token_str, line,
            |c| c.is_digit(radix)))),
      _ => Err("** Missing digits: ".to_string() + &token_str)
   }
}

fn build_float_part(token: ResultToken, line: &mut Line)
   -> ResultToken
{
   let result = build_point_float(token, line);
   let result = build_exp_float(result, line);
   let result = build_img_float(result, line);
   result
}

fn require_float_part(token: ResultToken, line: &mut Line)
   -> ResultToken
{
   let float_part;

   {
      let first = line.chars.peek();
      float_part = first.map_or(false,
         |&c| c == '.' || c == 'e' || c == 'E' || c == 'j' || c == 'J'
         );
   }

   if !float_part
   {
      Err("** missing float part: ".to_string() +
         &token.ok().unwrap().lexeme())

   }
   else
   {
      build_float_part(token, line)
   }
}

fn build_point_float(token: ResultToken, line: &mut Line)
   -> ResultToken
{
   if token.is_err()
   {
      return token;
   }

   if line.chars.peek().is_none() ||
      *line.chars.peek().unwrap() != '.'
   {
      return token;
   }

   match token
   {
      Ok(ref t) if t.is_decimal_integer() => (),
      _ => return Err(
         format!("Invalid floating point number: {:?}", token).to_string())
   }

   let mut token_str = token.ok().unwrap().lexeme();

   token_str.push(line.chars.next().unwrap());

   if line.chars.peek().map_or(false, |c| c.is_digit(10))
   {
      require_radix_digits(token_str, line, 10, |s| Token::Float(s))
   }
   else
   {
      Ok(Token::Float(token_str))
   }
}

fn build_exp_float(token: ResultToken, line: &mut Line)
   -> ResultToken
{
   if token.is_err()
   {
      return token;
   }

   if line.chars.peek().is_none() ||
      (*line.chars.peek().unwrap() != 'e' &&
      *line.chars.peek().unwrap() != 'E')
   {
      return token;
   }

   match token
   {
      Ok(ref t) if t.is_decimal_integer() || t.is_float() => (),
      _ => return Err(
         format!("Invalid floating point number: {:?}",
            token.ok().unwrap()).to_string()),
   }

   let mut token_str = token.ok().unwrap().lexeme();

   token_str.push(line.chars.next().unwrap()); // consume the e|E

   // plus or minus here
   if line.chars.peek().map_or(false, |&c| c == '+' || c == '-')
   {
      token_str.push(line.chars.next().unwrap()); // consume the +|-
   }

   require_radix_digits(token_str, line, 10, |s| Token::Float(s))
}

fn build_img_float(token: ResultToken, line: &mut Line)
   -> ResultToken
{
   if token.is_err()
   {
      return token;
   }

   if line.chars.peek().is_none() ||
      (*line.chars.peek().unwrap() != 'j' &&
      *line.chars.peek().unwrap() != 'J')
   {
      return token;
   }

   match token
   {
      Ok(ref t) if t.is_decimal_integer() || t.is_float() => (),
      _ => return Err(format!("Invalid imaginary number: {:?}", token)
            .to_string())
   }

   let mut token_str = token.ok().unwrap().lexeme();

   token_str.push(line.chars.next().unwrap()); // consume the j|J

   Ok(Token::Imaginary(token_str))
}

fn process_symbols(mut line: Line)
   -> (Option<(usize, ResultToken)>, Option<Line>)
{
   let result = build_symbol(&mut line);
   (Some(result), Some(line))
}

fn build_symbol(line: &mut Line)
   -> (usize, ResultToken)
{
   let result =
      match line.chars.peek()
      {
         Some(&'(') => match_one(line, Token::Lparen),
         Some(&')') => match_one(line, Token::Rparen),
         Some(&'[') => match_one(line, Token::Lbracket),
         Some(&']') => match_one(line, Token::Rbracket),
         Some(&'{') => match_one(line, Token::Lbrace),
         Some(&'}') => match_one(line, Token::Rbrace),
         Some(&',') => match_one(line, Token::Comma),
         Some(&':') => match_one(line, Token::Colon),
         Some(&';') => match_one(line, Token::Semi),
         Some(&'~') => match_one(line, Token::BitNot),
         Some(&'=') => match_pair_opt(
            match_one(line, Token::Assign), line, '=', Token::EQ),
         Some(&'@') => match_pair_opt(
            match_one(line, Token::At), line, '=', Token::AssignAt),
         Some(&'%') => match_pair_opt(
            match_one(line, Token::Mod), line, '=', Token::AssignMod),
         Some(&'&') => match_pair_opt(
            match_one(line, Token::BitAnd), line, '=', Token::AssignBitAnd),
         Some(&'|') => match_pair_opt(
            match_one(line, Token::BitOr), line, '=', Token::AssignBitOr),
         Some(&'^') => match_pair_opt(
            match_one(line, Token::BitXor), line, '=', Token::AssignBitXor),
         Some(&'+') => match_pair_opt(
            match_one(line, Token::Plus), line, '=', Token::AssignPlus),
         Some(&'*') =>
         {
            let token = match_one(line, Token::Times);
            match_pair_eq_opt(line, token, '*', Token::Exponent)
         },
         Some(&'/') =>
         {
            let token = match_one(line, Token::Divide);
            match_pair_eq_opt(line, token, '/', Token::DivideFloor)
         },
         Some(&'<') =>
         {
            let token = match_one(line, Token::LT);
            match_pair_eq_opt(line, token, '<', Token::Lshift)
         },
         Some(&'>') =>
         {
            let token = match_one(line, Token::GT);
            match_pair_eq_opt(line, token, '>', Token::Rshift)
         },
         Some(&'-') =>
         {
            let token = match_one(line, Token::Minus);
            let token = match_pair_opt(token, line, '=', Token::AssignMinus);
            if token == Token::Minus
            {
               match_pair_opt(token, line, '>', Token::Arrow)
            }
            else
            {
               token
            }
         },
         Some(&'!') =>
         {
            // consume character
            line.chars.next();
            match line.chars.peek()
            {
               Some(&'=') => match_one(line, Token::NE),
               _ => return (line.number, Err("** Solitary '!'".to_string())),
            }
         }
         Some(&'.') =>
         {
            // consume character
            line.chars.next();
            match (line.chars.peek(), line.chars.peek_second())
            {
               (Some(&'.'), Some(&'.')) =>
               {
                  line.chars.next();
                  line.chars.next();
                  Token::Ellipsis
               },
               _ => Token::Dot,
            }
         }
         Some(&c) => return (line.number,
            Err(format!("Unrecognized symbol '{}'", c).to_string())),
         _ => return (line.number, Err("internal error".to_string())),
      };

   (line.number, Ok(result))
}

fn match_one(line: &mut Line, tk: Token)
   -> Token
{
   line.chars.next();
   tk
}

fn match_pair_opt(old_token: Token, line: &mut Line,
   c: char, matched_token: Token)
   -> Token
{
   if line.chars.peek().map_or(false, |&k| k == c)
   {
      line.chars.next();
      matched_token
   }
   else
   {
      old_token
   }
}

fn match_pair_eq_opt(line: &mut Line, initial_token: Token,
   paired_char: char, paired_token: Token)
   -> Token
{
   let token = match_pair_opt(initial_token, line, paired_char, paired_token);
   let weq = token.with_equal();
   match_pair_opt(token, line, '=', weq)
}

fn consume_space_to_next(current_line: &mut Line)
{
   while current_line.chars.peek().map_or(false, |&c| is_space(c))
   {
      current_line.chars.next();
   }
}

fn process_newline(line: Line)
   -> (Option<(usize, ResultToken)>, Option<Line>)
{
   (Some((line.number, Ok(Token::Newline))), None)
}

fn consume_and_while<P>(mut token_str: String, line: &mut Line, predicate: P)
   -> String
      where P: Fn(char) -> bool
{
   token_str.push(line.chars.next().unwrap());

   while line.chars.peek().map_or(false, |&c| predicate(c))
   {
      token_str.push(line.chars.next().unwrap());
   }

   token_str
}

fn determine_spaces(char_count: u32, tab_stop_size: u32)
   -> u32
{
   tab_stop_size - char_count % tab_stop_size
}

/// This function currently considers \r as a whitespace character instead
/// of an old Mac end of line character.
fn is_space(c: char)
   -> bool
{
   c == ' ' || c == '\t' || c == '\x0C' || c == '\r' // ignore \r for now
}

fn process_character(count: u32, c: char)
   -> u32
{
   if c == '\t'
   {
      count + determine_spaces(count, TAB_STOP_SIZE)
   }
   else
   {
      count + 1
   }
}

fn count_indentation(chars: &mut DoublePeekable<Chars>)
   -> (u32, String)
{
   let mut count = 0;
   let mut spaces = String::new();

   while let Some(&c) = chars.peek()
   {
      if is_space(c)
      {
         count = process_character(count, c);
         spaces.push(chars.next().unwrap());
      }
      else
      {
         break;
      }
   }

   (count, spaces)
}

/// This function should be modified to do a more appropriate unicode
/// check.  Eliding for now due to apparently unstable support in Rust.
fn is_xid_start(c: char)
   -> bool
{
   c.is_alphabetic() || c == '_'
}

/// This function should be modified to do a more appropriate unicode
/// check.  Eliding for now due to apparently unstable support in Rust.
fn is_xid_continue(c: char)
   -> bool
{
   c.is_alphanumeric() || c == '_'
}


#[cfg(test)]
mod tests
{
   use super::Lexer;
   use tokens::Token;

   #[test]
   fn test_identifiers()
   {
      let chars = "abf  \x0C _xyz\n   \n  e2f\n  \tmq3\nn12\\\r\nn3\\ \n  n23\n    n24\n   n25     # monkey says what?  \n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("abf".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("_xyz".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((3, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((3, Ok(Token::Identifier("e2f".to_string())))));
      assert_eq!(l.next(), Some((3, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((4, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((4, Ok(Token::Identifier("mq3".to_string())))));
      assert_eq!(l.next(), Some((4, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((5, Ok(Token::Dedent))));
      assert_eq!(l.next(), Some((5, Ok(Token::Dedent))));
      assert_eq!(l.next(), Some((5, Ok(Token::Identifier("n12".to_string())))));
      assert_eq!(l.next(), Some((6, Ok(Token::Identifier("n3".to_string())))));
      assert_eq!(l.next(), Some((6, Err("** bad \\ **".to_string()))));
      assert_eq!(l.next(), Some((6, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((7, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((7, Ok(Token::Identifier("n23".to_string())))));
      assert_eq!(l.next(), Some((7, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((8, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((8, Ok(Token::Identifier("n24".to_string())))));
      assert_eq!(l.next(), Some((8, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((9, Err("** DEDENT ERROR **".to_string()))));
      assert_eq!(l.next(), Some((9, Ok(Token::Identifier("n25".to_string())))));
      assert_eq!(l.next(), Some((9, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((0, Ok(Token::Dedent))));
      assert_eq!(l.next(), Some((0, Ok(Token::Dedent))));
   }   

   #[test]
   fn test_numbers()
   {
      let chars = "1 123 456 45 23.742 23. 12..3 .14 0123.2192 077e010 12e17 12e+17 12E-17 0 00000 00003 0.2 .e12 0o724 0X32facb7 0b10101010 0x 00000e+00000 79228162514264337593543950336 0xdeadbeef 037j 2.3j 2.j .3j . 3..2\n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("1".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("123".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("456".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("45".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("23.742".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("23.".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("12.".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float(".3".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float(".14".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("0123.2192".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("077e010".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("12e17".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("12e+17".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("12E-17".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("0".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("00000".to_string())))));
      assert_eq!(l.next(), Some((1, Err("** missing float part: 00003".to_string()))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("0.2".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Dot))));
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("e12".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::OctInteger("0o724".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::HexInteger("0X32facb7".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::BinInteger("0b10101010".to_string())))));
      assert_eq!(l.next(), Some((1, Err("** Missing digits: 0x".to_string()))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("00000e+00000".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::DecInteger("79228162514264337593543950336".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::HexInteger("0xdeadbeef".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Imaginary("037j".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Imaginary("2.3j".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Imaginary("2.j".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Imaginary(".3j".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Dot))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float("3.".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Float(".2".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
   }   

   #[test]
   fn test_dedent()
   {
      let chars = "    abf xyz\n\n\n\n        e2f\n             n12\n  n2\n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("abf".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("xyz".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((5, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((5, Ok(Token::Identifier("e2f".to_string())))));
      assert_eq!(l.next(), Some((5, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((6, Ok(Token::Indent))));
      assert_eq!(l.next(), Some((6, Ok(Token::Identifier("n12".to_string())))));
      assert_eq!(l.next(), Some((6, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((7, Ok(Token::Dedent))));
      assert_eq!(l.next(), Some((7, Ok(Token::Dedent))));
      assert_eq!(l.next(), Some((7, Err("** DEDENT ERROR **".to_string()))));
      assert_eq!(l.next(), Some((7, Ok(Token::Identifier("n2".to_string())))));
      assert_eq!(l.next(), Some((7, Ok(Token::Newline))));
   }   

   #[test]
   fn test_symbols()
   {
      let chars = "(){}[]:,.;..===@->+=-=*=/=//=%=@=&=|=^=>>=<<=**=+-***///%@<<>>&|^~<><=>===!=!...";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::Lparen))));
      assert_eq!(l.next(), Some((1, Ok(Token::Rparen))));
      assert_eq!(l.next(), Some((1, Ok(Token::Lbrace))));
      assert_eq!(l.next(), Some((1, Ok(Token::Rbrace))));
      assert_eq!(l.next(), Some((1, Ok(Token::Lbracket))));
      assert_eq!(l.next(), Some((1, Ok(Token::Rbracket))));
      assert_eq!(l.next(), Some((1, Ok(Token::Colon))));
      assert_eq!(l.next(), Some((1, Ok(Token::Comma))));
      assert_eq!(l.next(), Some((1, Ok(Token::Dot))));
      assert_eq!(l.next(), Some((1, Ok(Token::Semi))));
      assert_eq!(l.next(), Some((1, Ok(Token::Dot))));
      assert_eq!(l.next(), Some((1, Ok(Token::Dot))));
      assert_eq!(l.next(), Some((1, Ok(Token::EQ))));
      assert_eq!(l.next(), Some((1, Ok(Token::Assign))));
      assert_eq!(l.next(), Some((1, Ok(Token::At))));
      assert_eq!(l.next(), Some((1, Ok(Token::Arrow))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignPlus))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignMinus))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignTimes))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignDivide))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignDivideFloor))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignMod))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignAt))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignBitAnd))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignBitOr))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignBitXor))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignRshift))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignLshift))));
      assert_eq!(l.next(), Some((1, Ok(Token::AssignExponent))));
      assert_eq!(l.next(), Some((1, Ok(Token::Plus))));
      assert_eq!(l.next(), Some((1, Ok(Token::Minus))));
      assert_eq!(l.next(), Some((1, Ok(Token::Exponent))));
      assert_eq!(l.next(), Some((1, Ok(Token::Times))));
      assert_eq!(l.next(), Some((1, Ok(Token::DivideFloor))));
      assert_eq!(l.next(), Some((1, Ok(Token::Divide))));
      assert_eq!(l.next(), Some((1, Ok(Token::Mod))));
      assert_eq!(l.next(), Some((1, Ok(Token::At))));
      assert_eq!(l.next(), Some((1, Ok(Token::Lshift))));
      assert_eq!(l.next(), Some((1, Ok(Token::Rshift))));
      assert_eq!(l.next(), Some((1, Ok(Token::BitAnd))));
      assert_eq!(l.next(), Some((1, Ok(Token::BitOr))));
      assert_eq!(l.next(), Some((1, Ok(Token::BitXor))));
      assert_eq!(l.next(), Some((1, Ok(Token::BitNot))));
      assert_eq!(l.next(), Some((1, Ok(Token::LT))));
      assert_eq!(l.next(), Some((1, Ok(Token::GT))));
      assert_eq!(l.next(), Some((1, Ok(Token::LE))));
      assert_eq!(l.next(), Some((1, Ok(Token::GE))));
      assert_eq!(l.next(), Some((1, Ok(Token::EQ))));
      assert_eq!(l.next(), Some((1, Ok(Token::NE))));
      assert_eq!(l.next(), Some((1, Err("** Solitary '!'".to_string()))));
      assert_eq!(l.next(), Some((1, Ok(Token::Ellipsis))));
   }

   #[test]
   fn test_keywords()
   {
      let chars = "false False None True and as assert break class continue def del defdel elif else except finally for from \nglobal if import in is lambda nonlocal not or pass raise return try while with yield\n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("false".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::False))));
      assert_eq!(l.next(), Some((1, Ok(Token::None))));
      assert_eq!(l.next(), Some((1, Ok(Token::True))));
      assert_eq!(l.next(), Some((1, Ok(Token::And))));
      assert_eq!(l.next(), Some((1, Ok(Token::As))));
      assert_eq!(l.next(), Some((1, Ok(Token::Assert))));
      assert_eq!(l.next(), Some((1, Ok(Token::Break))));
      assert_eq!(l.next(), Some((1, Ok(Token::Class))));
      assert_eq!(l.next(), Some((1, Ok(Token::Continue))));
      assert_eq!(l.next(), Some((1, Ok(Token::Def))));
      assert_eq!(l.next(), Some((1, Ok(Token::Del))));
      assert_eq!(l.next(), Some((1, Ok(Token::Identifier("defdel".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Elif))));
      assert_eq!(l.next(), Some((1, Ok(Token::Else))));
      assert_eq!(l.next(), Some((1, Ok(Token::Except))));
      assert_eq!(l.next(), Some((1, Ok(Token::Finally))));
      assert_eq!(l.next(), Some((1, Ok(Token::For))));
      assert_eq!(l.next(), Some((1, Ok(Token::From))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((2, Ok(Token::Global))));
      assert_eq!(l.next(), Some((2, Ok(Token::If))));
      assert_eq!(l.next(), Some((2, Ok(Token::Import))));
      assert_eq!(l.next(), Some((2, Ok(Token::In))));
      assert_eq!(l.next(), Some((2, Ok(Token::Is))));
      assert_eq!(l.next(), Some((2, Ok(Token::Lambda))));
      assert_eq!(l.next(), Some((2, Ok(Token::Nonlocal))));
      assert_eq!(l.next(), Some((2, Ok(Token::Not))));
      assert_eq!(l.next(), Some((2, Ok(Token::Or))));
      assert_eq!(l.next(), Some((2, Ok(Token::Pass))));
      assert_eq!(l.next(), Some((2, Ok(Token::Raise))));
      assert_eq!(l.next(), Some((2, Ok(Token::Return))));
      assert_eq!(l.next(), Some((2, Ok(Token::Try))));
      assert_eq!(l.next(), Some((2, Ok(Token::While))));
      assert_eq!(l.next(), Some((2, Ok(Token::With))));
      assert_eq!(l.next(), Some((2, Ok(Token::Yield))));
      assert_eq!(l.next(), Some((2, Ok(Token::Newline))));
   }

   #[test]
   fn test_strings_1()
   {
      let chars = "'abc 123 \txyz@\")#*)@'\n\"wfe wf w fwe'fwefw\"\n\"abc\n'last line'\n'just\\\n   kidding   \\\n \t kids'\n'xy\\\n  zq\\\n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::String("abc 123 \txyz@\")#*)@".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((2, Ok(Token::String("wfe wf w fwe'fwefw".to_string())))));
      assert_eq!(l.next(), Some((2, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((3, Err("unterminated \"".to_string()))));
      assert_eq!(l.next(), Some((3, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((4, Ok(Token::String("last line".to_string())))));
      assert_eq!(l.next(), Some((4, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((5, Ok(Token::String("just   kidding    \t kids".to_string())))));
      assert_eq!(l.next(), Some((7, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((10, Err("unterminated \'".to_string()))));
   }

   #[test]
   fn test_strings_2()
   {
      let chars = "'abc' \"def\" \\\n'123'\n";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::String("abcdef123".to_string())))));
      assert_eq!(l.next(), Some((2, Ok(Token::Newline))));
   }

   #[test]
   fn test_strings_3()
   {
      let chars = "''' abc ' '' '''\n\"\"\"xyz\"\"\"\n'''abc\n \tdef\n123'''\n'''abc\\\n \tdef\\\n123'''\n'''abc\ndef";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::String(" abc ' '' ".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((2, Ok(Token::String("xyz".to_string())))));
      assert_eq!(l.next(), Some((2, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((3, Ok(Token::String("abc\n \tdef\n123".to_string())))));
      assert_eq!(l.next(), Some((5, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((6, Ok(Token::String("abc \tdef123".to_string())))));
      assert_eq!(l.next(), Some((8, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((11, Err("unterminated '''".to_string()))));
   }

   #[test]
   fn test_strings_4()
   {
      let chars = "'\\\\'\n'\\''\n'\\\"'\n'\\a'\n'\\b'\n'\\f'\n'\\n'\n'\\r'\n'\\t'\n'\\v'";
      let mut l = Lexer::new(chars.lines_any());
      assert_eq!(l.next(), Some((1, Ok(Token::String("\\".to_string())))));
      assert_eq!(l.next(), Some((1, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((2, Ok(Token::String("'".to_string())))));
      assert_eq!(l.next(), Some((2, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((3, Ok(Token::String("\"".to_string())))));
      assert_eq!(l.next(), Some((3, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((4, Ok(Token::String("\x07".to_string())))));
      assert_eq!(l.next(), Some((4, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((5, Ok(Token::String("\x08".to_string())))));
      assert_eq!(l.next(), Some((5, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((6, Ok(Token::String("\x0C".to_string())))));
      assert_eq!(l.next(), Some((6, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((7, Ok(Token::String("\n".to_string())))));
      assert_eq!(l.next(), Some((7, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((8, Ok(Token::String("\r".to_string())))));
      assert_eq!(l.next(), Some((8, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((9, Ok(Token::String("\t".to_string())))));
      assert_eq!(l.next(), Some((9, Ok(Token::Newline))));
      assert_eq!(l.next(), Some((10, Ok(Token::String("\x0B".to_string())))));
   }
}
