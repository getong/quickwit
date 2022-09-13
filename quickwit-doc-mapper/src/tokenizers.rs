// Copyright (C) 2022 Quickwit, Inc.
//
// Quickwit is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at hello@quickwit.io.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

use std::str::CharIndices;

use once_cell::sync::Lazy;
use regex::Regex;
use tantivy::tokenizer::{
    BoxTokenStream, RawTokenizer, RemoveLongFilter, TextAnalyzer, Token, TokenStream, Tokenizer,
    TokenizerManager,
};

static REGEX_ARRAY: Lazy<[Regex; 3]> = Lazy::new(|| {
    [
        // URL regex to match http(s) links or www links
        Regex::new("^[a-z]+:?/?/?[a-z0-9]+[-_\\./a-z]*\\.[a-z]+[-/a-z._=]+").expect("Failed to compile regular expression. This should never happen! Please, report on https://github.com/quickwit-oss/quickwit/issues."),

        // Date regex to match dates similar to this example "Dec 10 06:55:48",
        Regex::new(
            "^[A-Za-z]{1,3}[ \t]{1,2}[0-9]{1,2}[ \
             \t]{1,2}[0-9]{1,4}[-_/:][0-9]{1,2}[-_/:][0-9]{1,4}",
             )
            .expect("Failed to compile regular expression. This should never happen! Please, report on https://github.com/quickwit-oss/quickwit/issues."),

        // Regex to match numbers or numbers with certain chars (e.g IP addresses)
        Regex::new("^[0-9][-/%_\\.:a-zA-Z0-9]*").expect("Failed to compile regular expression. This should never happen! Please, report on https://github.com/quickwit-oss/quickwit/issues.")
    ]
});

static VALID_CHARS: &str = ".-/:%";

/// Log friendly Tokenizer that avoids splittings on ponctuation in:
///     - IP addresses (both ipv4 and ipv6)
#[derive(Clone)]
pub struct LogTokenizer;

#[allow(missing_docs)]
pub struct LogTokenStream<'a> {
    text: &'a str,
    chars: CharIndices<'a>,
    token: Token,
}

impl Tokenizer for LogTokenizer {
    fn token_stream<'a>(&self, text: &'a str) -> BoxTokenStream<'a> {
        BoxTokenStream::from(LogTokenStream {
            text,
            chars: text.char_indices(),
            token: Token::default(),
        })
    }
}

impl<'a> LogTokenStream<'a> {
    // Characters we probably don't want to split on aside from alphanumeric
    fn search_token_end(&mut self) -> usize {
        (&mut self.chars)
            .filter(|&(_, ref c)| !VALID_CHARS.contains(*c) && !c.is_alphanumeric())
            .map(|(offset, _)| offset)
            .next()
            .unwrap_or(self.text.len())
    }

    fn handle_match(&mut self, offset_to: usize) -> usize {
        (&mut self.chars)
            .filter(|(index, _)| *index == offset_to)
            .map(|(offset, _)| offset)
            .next()
            .unwrap_or(self.text.len())
    }

    fn push_token(&mut self, offset_from: usize, offset_to: usize) {
        self.token.offset_from = offset_from;
        self.token.offset_to = offset_to;
        self.token.text.push_str(&self.text[offset_from..offset_to]);
    }
}

impl<'a> TokenStream for LogTokenStream<'a> {
    fn advance(&mut self) -> bool {
        self.token.text.clear();
        self.token.position = self.token.position.wrapping_add(1);

        // TODO Replace iterator with index maybe ?
        while let Some((offset_from, c)) = self.chars.next() {
            let text_substring = &self.text[offset_from..];

            for regex in REGEX_ARRAY.iter() {
                if let Some(regex_match) = regex.find(text_substring) {
                    let offset_to = self.handle_match(offset_from + regex_match.end());
                    self.push_token(offset_from, offset_to);

                    return true;
                }
            }

            // Catch all condition for words and '/' in case we stumble upon a
            // filesystem path or endpoints
            if c.is_alphabetic() || c == '/' {
                let offset_to = self.search_token_end();
                self.push_token(offset_from, offset_to);

                return true;
            }
        }

        false
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

fn get_quickwit_tokenizer_manager() -> TokenizerManager {
    let raw_tokenizer = TextAnalyzer::from(RawTokenizer).filter(RemoveLongFilter::limit(100));

    // TODO eventually check for other restrictions
    let log_tokenizer = TextAnalyzer::from(LogTokenizer).filter(RemoveLongFilter::limit(100));

    let tokenizer_manager = TokenizerManager::default();
    tokenizer_manager.register("raw", raw_tokenizer);
    tokenizer_manager.register("log", log_tokenizer);
    tokenizer_manager
}

/// Quickwits default tokenizer
pub static QUICKWIT_TOKENIZER_MANAGER: Lazy<TokenizerManager> =
    Lazy::new(get_quickwit_tokenizer_manager);

#[test]
fn raw_tokenizer_test() {
    let my_haiku = r#"
        white sandy beach
        a strong wind is coming
        sand in my face
        "#;
    let my_long_text = "a text, that is just too long, no one will type it, no one will like it, \
                        no one shall find it. I just need some more chars, now you may not pass.";

    let tokenizer = get_quickwit_tokenizer_manager().get("raw").unwrap();
    let mut haiku_stream = tokenizer.token_stream(my_haiku);
    assert!(haiku_stream.advance());
    assert!(!haiku_stream.advance());
    assert!(!tokenizer.token_stream(my_long_text).advance());
}

#[cfg(test)]
mod tests {
    use tantivy::tokenizer::{SimpleTokenizer, TextAnalyzer};

    use crate::tokenizers::get_quickwit_tokenizer_manager;

    #[test]
    fn log_tokenizer_basic_test() {
        let numbers = "255.255.255.255 test \n\ttest\t 27-05-2022 \t\t  \n \tat\r\n 02:51";
        let array_ref: [&str; 6] = [
            "255.255.255.255",
            "test",
            "test",
            "27-05-2022",
            "at",
            "02:51",
        ];

        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(numbers);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }

    // The only difference with the default tantivy is within numbers, this test is
    // to check if the behaviour is affected
    #[test]
    fn log_tokenizer_compare_with_simple() {
        let test_string = "this,is,the,test 42 here\n3932\t20dk,3093raopxa'wd";
        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);
        let mut ref_token_stream = TextAnalyzer::from(SimpleTokenizer).token_stream(test_string);

        while token_stream.advance() && ref_token_stream.advance() {
            assert_eq!(&token_stream.token().text, &ref_token_stream.token().text);
        }

        assert!(!(token_stream.advance() || ref_token_stream.advance()));
    }

    #[test]
    fn log_tokenizer_log_test() {
        let test_string = "Dec 10 06:55:48 LabSZ sshd[24200]: Failed password for invalid user \
                           webmaster from 173.234.31.186 port 38926 ssh2";
        let array_ref: [&str; 15] = [
            "Dec 10 06:55:48",
            "LabSZ",
            "sshd",
            "24200",
            "Failed",
            "password",
            "for",
            "invalid",
            "user",
            "webmaster",
            "from",
            "173.234.31.186",
            "port",
            "38926",
            "ssh2",
        ];

        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }

    #[test]
    fn log_tokenizer_log_2() {
        let test_string = "1331901000.000000    CHEt7z3AzG4gyCNgci    192.168.202.79    50465    \
                           192.168.229.251    80    1    HEAD 192.168.229.251    /DEASLog02.nsf    \
                           -    Mozilla/5.0";
        let array_ref: [&str; 11] = [
            "1331901000.000000",
            "CHEt7z3AzG4gyCNgci",
            "192.168.202.79",
            "50465",
            "192.168.229.251",
            "80",
            "1",
            "HEAD",
            "192.168.229.251",
            "/DEASLog02.nsf",
            "Mozilla/5.0",
        ];

        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }

    #[test]
    fn log_tokenizer_log_test_http() {
        let test_string = "{\"message\" : \"211.11.9.0 - - [1998-06-21T15:00:01-05:00] \"GET \
                           /english/index.html HTTP/1.0\" 304 0\"}";
        let array_ref: [&str; 8] = [
            "message",
            "211.11.9.0",
            "1998-06-21T15:00:01-05:00",
            "GET",
            "/english/index.html",
            "HTTP/1.0",
            "304",
            "0",
        ];
        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }

    #[test]
    fn log_tokenizer_ip_test() {
        let test_string = r"255.255.255.255
            0f31:e019:5e74:6679:3134:99f1:8f55:fa2a
            e6c5:5182:b404:7e64:d91f:ba40:bfb7:c184
            12.32.75.221
            ";

        let array_ref: [&str; 4] = [
            "255.255.255.255",
            "0f31:e019:5e74:6679:3134:99f1:8f55:fa2a",
            "e6c5:5182:b404:7e64:d91f:ba40:bfb7:c184",
            "12.32.75.221"
        ];

        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }

    #[test]
    fn log_tokenizer_log_wsa() {
        let test_string = "54.36.149.41 - - [22/Jan/2019:03:56:14 +0330] \"GET /filter/27|13%20%D9%85%DA%AF%D8%A7%D9%BE%DB%8C%DA%A9%D8%B3%D9%84,27|%DA%A9%D9%85%D8%AA%D8%B1%20%D8%A7%D8%B2%205%20%D9%85%DA%AF%D8%A7%D9%BE%DB%8C%DA%A9%D8%B3%D9%84,p53 HTTP/1.1\" 200 30577 \"-\" \"Mozilla/5.0 (compatible; AhrefsBot/6.1; +http://ahrefs.com/robot/)\" \"-\"";

        let array_ref: [&str; 16] = [
            "54.36.149.41",
            "22/Jan/2019:03:56:14",
            "0330",
            "GET",
            "/filter/27",
            "13%20%D9%85%DA%AF%D8%A7%D9%BE%DB%8C%DA%A9%D8%B3%D9%84",
            "27",
            "DA%A9%D9%85%D8%AA%D8%B1%20%D8%A7%D8%B2%205%20%D9%85%DA%AF%D8%A7%D9%BE%DB%8C%DA%A9%D8%\
                B3%D9%84",
                "p53",
                "HTTP/1.1",
                "200",
                "30577",
                "Mozilla/5.0",
                "compatible",
                "AhrefsBot/6.1",
                "http://ahrefs.com/robot/",
        ];

        let mut token_stream = get_quickwit_tokenizer_manager()
            .get("log")
            .unwrap()
            .token_stream(test_string);

        array_ref.iter().for_each(|ref_token| {
            if token_stream.advance() {
                assert_eq!(&token_stream.token().text, ref_token)
            } else {
                panic!()
            }
        });
    }
}
