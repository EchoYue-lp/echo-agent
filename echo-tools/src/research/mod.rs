//! Research and paper retrieval tools.
//!
//! Provides academic paper search (arxiv, Semantic Scholar),
//! PDF download + parse, and BibTeX generation.

pub mod arxiv;
pub mod bibtex;
pub mod pdf_fetch;
pub mod semantic_scholar;

pub use arxiv::ArxivSearchTool;
pub use bibtex::BibtexGenerateTool;
pub use pdf_fetch::PdfFetchTool;
pub use semantic_scholar::SemanticScholarSearchTool;
