//! Vault search's full-text index: tantivy (ADR 0006) over notes' titles,
//! text, tags and folders, kept in memory or in a cache folder on disk. It
//! knows nothing of vaults: orbit-core feeds it notes and asks it queries.

use std::{collections::HashMap, fs, io, ops::Range, path::Path, sync::Mutex};

use tantivy::collector::{Count, TopDocs};
use tantivy::query::{AllQuery, BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, TermQuery};
use tantivy::schema::{
    FAST, Field, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing,
    TextOptions, Value as _,
};
use tantivy::tokenizer::{AsciiFoldingFilter, LowerCaser, SimpleTokenizer, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

/// The tokenizer for titles and text: words, lowercased, accents folded
/// (`Café` is `cafe`).
const WORDS: &str = "orbit_words";
/// How much more a word in a note's title counts than one in its text.
const TITLE_BOOST: f32 = 3.;
/// What a word only begun (`road` for `roadmap`) adds to a note's score.
const PREFIX_SCORE: f32 = 0.5;
/// The writer's memory before it flushes to disk.
const WRITER_MEMORY: usize = 50_000_000;

/// A note as the index takes it.
pub struct NoteDoc {
    /// Its vault path, which names it in the index.
    pub path: String,
    /// What it's called: its title and aliases.
    pub title: String,
    /// Its text, without front matter.
    pub body: String,
    /// Its tags and every tag they're nested in, lowercased, without `#`.
    pub tags: Vec<String>,
    /// Every folder it's in, lowercased, from the vault root down.
    pub folders: Vec<String>,
    /// When it last changed, for knowing whether it has since.
    pub modified: u64,
}

/// A search: words to find and filters to narrow by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The words typed, as typed.
    pub words: String,
    /// `tag:` filters: lowercased, without `#`.
    pub tags: Vec<String>,
    /// `in:` filters: folders, lowercased.
    pub folders: Vec<String>,
}

impl Query {
    /// Reads `tag:planning in:Projects some words`; a filter's value can be
    /// quoted (`in:"Two words"`).
    pub fn parse(input: &str) -> Query {
        let mut query = Query::default();
        let mut words = Vec::new();
        for part in split_quoted(input) {
            let filter = part.split_once(':').filter(|(key, value)| {
                !value.is_empty() && matches!(key.to_ascii_lowercase().as_str(), "tag" | "in")
            });
            match filter {
                Some((key, value)) => {
                    let value = value.trim_matches('"').to_lowercase();
                    if key.eq_ignore_ascii_case("tag") {
                        query
                            .tags
                            .push(value.trim_start_matches('#').trim_end_matches('/').into());
                    } else {
                        query.folders.push(value.trim_matches('/').into());
                    }
                }
                None => words.push(part),
            }
        }
        query.words = words.join(" ");
        query
    }
}

/// Splits at spaces outside double quotes.
fn split_quoted(input: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted) = (None, false);
    for (i, c) in input.char_indices() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if let Some(s) = start.take() {
                    parts.push(&input[s..i]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    parts.extend(start.map(|s| &input[s..]));
    parts
}

/// A note a search found.
pub struct Hit {
    pub path: String,
    /// Its text, as indexed.
    pub body: String,
    /// Where the words searched for are in `body`, in bytes.
    pub matches: Vec<Range<usize>>,
}

/// What a search found.
pub struct Hits {
    /// The best ones, best first.
    pub hits: Vec<Hit>,
    /// How many notes match in all.
    pub total: usize,
}

/// The schema's fields.
struct Fields {
    path: Field,
    title: Field,
    body: Field,
    tags: Field,
    folders: Field,
    modified: Field,
}

/// The index, with what reads and writes it: shared between threads.
pub struct SearchIndex {
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: Fields,
    words: TextAnalyzer,
}

impl SearchIndex {
    /// The index kept in `dir`, made there if it isn't (or can't be read,
    /// or is from another version of Orbit); or, without a folder, one in
    /// memory.
    pub fn open(dir: Option<&Path>) -> tantivy::Result<SearchIndex> {
        let (schema, fields) = schema();
        let index = match dir {
            None => Index::create_in_ram(schema),
            Some(dir) => match open_in(dir, &schema) {
                Ok(index) => index,
                // A cache from another version, or damaged, is made again;
                // one that can't be reached now (an IO error) is left alone.
                Err(e @ tantivy::TantivyError::IoError(_)) => return Err(e),
                Err(_) => {
                    fs::remove_dir_all(dir).or_else(not_found_is_fine)?;
                    open_in(dir, &schema)?
                }
            },
        };
        let words = TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(LowerCaser)
            .filter(AsciiFoldingFilter)
            .build();
        index.tokenizers().register(WORDS, words.clone());
        let writer = index.writer_with_num_threads(1, WRITER_MEMORY)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(SearchIndex {
            reader,
            writer: Mutex::new(writer),
            fields,
            words,
        })
    }

    /// Every note in the index, with when it last changed as indexed.
    pub fn modified(&self) -> tantivy::Result<HashMap<String, u64>> {
        let mut found = HashMap::new();
        let searcher = self.reader.searcher();
        for segment in searcher.segment_readers() {
            let fast = segment.fast_fields();
            let (Some(paths), modified) = (fast.str("path")?, fast.u64("modified")?) else {
                continue;
            };
            let alive = segment.alive_bitset();
            let mut path = String::new();
            for doc in 0..segment.max_doc() {
                if alive.is_some_and(|alive| !alive.is_alive(doc)) {
                    continue;
                }
                let Some(ord) = paths.term_ords(doc).next() else {
                    continue;
                };
                path.clear();
                paths.ord_to_str(ord, &mut path)?;
                found.insert(path.clone(), modified.first(doc).unwrap_or(0));
            }
        }
        Ok(found)
    }

    /// Adds a note, or replaces it if it's in the index already. Searches
    /// see it after [`Self::commit`].
    pub fn put(&self, note: NoteDoc) -> tantivy::Result<()> {
        let f = &self.fields;
        let mut doc = TantivyDocument::default();
        doc.add_text(f.path, &note.path);
        doc.add_text(f.title, &note.title);
        doc.add_text(f.body, &note.body);
        for tag in &note.tags {
            doc.add_text(f.tags, tag);
        }
        for folder in &note.folders {
            doc.add_text(f.folders, folder);
        }
        doc.add_u64(f.modified, note.modified);
        let writer = self.writer();
        writer.delete_term(Term::from_field_text(f.path, &note.path));
        writer.add_document(doc)?;
        Ok(())
    }

    /// Takes a note out, from [`Self::commit`] on.
    pub fn remove(&self, path: &str) {
        self.writer()
            .delete_term(Term::from_field_text(self.fields.path, path));
    }

    /// Makes what was put and removed since the last commit searchable,
    /// and saved if the index is on disk.
    pub fn commit(&self) -> tantivy::Result<()> {
        self.writer().commit()?;
        self.reader.reload()
    }

    /// Up to `limit` notes with every word (the last one may be unfinished)
    /// in their title or text, and every filter; best first. Filters alone
    /// list every note they match.
    pub fn search(&self, query: &Query, limit: usize) -> tantivy::Result<Hits> {
        let terms = self.terms(&query.words);
        if terms.is_empty() && query.tags.is_empty() && query.folders.is_empty() {
            return Ok(Hits {
                hits: Vec::new(),
                total: 0,
            });
        }
        let f = &self.fields;
        let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
        for (i, term) in terms.iter().enumerate() {
            let last = i + 1 == terms.len();
            let mut word: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
            for (field, boost) in [(f.title, TITLE_BOOST), (f.body, 1.)] {
                let exact = TermQuery::new(
                    Term::from_field_text(field, term),
                    IndexRecordOption::WithFreqs,
                );
                word.push((
                    Occur::Should,
                    Box::new(BoostQuery::new(Box::new(exact), boost)),
                ));
                if last {
                    let prefix =
                        FuzzyTermQuery::new_prefix(Term::from_field_text(field, term), 0, true);
                    let score = PREFIX_SCORE * boost;
                    word.push((
                        Occur::Should,
                        Box::new(BoostQuery::new(Box::new(prefix), score)),
                    ));
                }
            }
            clauses.push((Occur::Must, Box::new(BooleanQuery::new(word))));
        }
        let filters = query
            .tags
            .iter()
            .map(|tag| Term::from_field_text(f.tags, tag))
            .chain(
                query
                    .folders
                    .iter()
                    .map(|folder| Term::from_field_text(f.folders, folder)),
            );
        for filter in filters {
            let filter = TermQuery::new(filter, IndexRecordOption::Basic);
            clauses.push((Occur::Must, Box::new(filter)));
        }
        if terms.is_empty() {
            clauses.push((Occur::Must, Box::new(AllQuery)));
        }
        let searcher = self.reader.searcher();
        let (top, total) = searcher.search(
            &BooleanQuery::new(clauses),
            &(TopDocs::with_limit(limit).order_by_score(), Count),
        )?;
        let mut hits = Vec::new();
        for (_, address) in top {
            let doc: TantivyDocument = searcher.doc(address)?;
            let text = |field| doc.get_first(field).and_then(|v| v.as_str()).unwrap_or("");
            let body = text(f.body).to_string();
            hits.push(Hit {
                path: text(f.path).to_string(),
                matches: self.matches(&body, &terms),
                body,
            });
        }
        Ok(Hits { hits, total })
    }

    /// The words of `text` as the index has them.
    fn terms(&self, text: &str) -> Vec<String> {
        let mut words = self.words.clone();
        let mut stream = words.token_stream(text);
        let mut terms = Vec::new();
        while stream.advance() {
            terms.push(stream.token().text.clone());
        }
        terms
    }

    /// Where `terms` are in `body`: the last one as a prefix too.
    fn matches(&self, body: &str, terms: &[String]) -> Vec<Range<usize>> {
        let Some((last, whole)) = terms.split_last() else {
            return Vec::new();
        };
        let mut words = self.words.clone();
        let mut stream = words.token_stream(body);
        let mut found = Vec::new();
        while stream.advance() {
            let token = stream.token();
            if whole.contains(&token.text) || token.text.starts_with(last.as_str()) {
                found.push(token.offset_from..token.offset_to);
            }
        }
        found
    }

    fn writer(&self) -> std::sync::MutexGuard<'_, IndexWriter> {
        self.writer.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn schema() -> (Schema, Fields) {
    let mut schema = Schema::builder();
    let words = |stored: bool| {
        let indexing = TextFieldIndexing::default()
            .set_tokenizer(WORDS)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let options = TextOptions::default().set_indexing_options(indexing);
        if stored {
            options.set_stored()
        } else {
            options
        }
    };
    let fields = Fields {
        path: schema.add_text_field("path", STRING | STORED | FAST),
        title: schema.add_text_field("title", words(false)),
        body: schema.add_text_field("body", words(true)),
        tags: schema.add_text_field("tags", STRING),
        folders: schema.add_text_field("folders", STRING),
        modified: schema.add_u64_field("modified", INDEXED | FAST),
    };
    (schema.build(), fields)
}

fn open_in(dir: &Path, schema: &Schema) -> tantivy::Result<Index> {
    fs::create_dir_all(dir)?;
    let directory = tantivy::directory::MmapDirectory::open(dir)?;
    let index = Index::open_or_create(directory, schema.clone())?;
    if index.schema() != *schema {
        return Err(tantivy::TantivyError::SchemaError(
            "made by another version of Orbit".into(),
        ));
    }
    Ok(index)
}

fn not_found_is_fine(e: io::Error) -> io::Result<()> {
    if e.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}
