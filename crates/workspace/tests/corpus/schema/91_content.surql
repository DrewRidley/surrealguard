-- Content tables for the wide-coverage half of the corpus: a full-text
-- analyzer and FULLTEXT index (`@@` / `search::*`), an HNSW index over an
-- `array<float>` field (`<|k, ef|>`), a UNIQUE index, a plain index for
-- `WITH INDEX` hints, and every field clause a query can meet — `ASSERT`,
-- `DEFAULT`, `VALUE`, `READONLY`, `COMPUTED` — on one table.
--
-- Everything here is verified against a live SurrealDB 3.2.3, which is why
-- the index says `FULLTEXT ANALYZER` (the 2.x `SEARCH ANALYZER` spelling is a
-- parse error there) and why there is no MTREE index (also gone in 3.x).

DEFINE ANALYZER english TOKENIZERS blank, class, punct FILTERS lowercase, snowball(english);

DEFINE TABLE article SCHEMAFULL;

DEFINE FIELD title ON article TYPE string;

-- `VALUE` normalises every write: the stored slug is always `string::slug` of
-- what was written.
DEFINE FIELD slug ON article TYPE string VALUE string::slug($value);

DEFINE FIELD body ON article TYPE string;

DEFINE FIELD author ON article TYPE record<account>;

DEFINE FIELD tags ON article TYPE array<string> DEFAULT [];

-- The vector the HNSW index is built over. Three dimensions keep the corpus
-- literals short.
DEFINE FIELD embedding ON article TYPE array<float>;

DEFINE FIELD views ON article TYPE int DEFAULT 0 ASSERT $value >= 0;

DEFINE FIELD rating ON article TYPE option<float> ASSERT $value = NONE OR ($value >= 0 AND $value <= 5);

DEFINE FIELD published_at ON article TYPE option<datetime>;

DEFINE FIELD location ON article TYPE option<geometry<point>>;

-- Written once, at creation, and never again (2025 owns the contract).
DEFINE FIELD created_at ON article TYPE datetime READONLY DEFAULT time::now();

DEFINE FIELD updated_at ON article TYPE datetime VALUE time::now();

DEFINE FIELD word_count ON article COMPUTED string::words($this.body).len();

DEFINE FIELD is_published ON article COMPUTED $this.published_at != NONE;

DEFINE INDEX article_slug ON article FIELDS slug UNIQUE;

DEFINE INDEX article_author ON article FIELDS author;

DEFINE INDEX article_body_search ON article FIELDS body FULLTEXT ANALYZER english BM25 HIGHLIGHTS;

DEFINE INDEX article_embedding ON article FIELDS embedding HNSW DIMENSION 3 DIST COSINE;
