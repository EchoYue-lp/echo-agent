//! Research and paper retrieval tools.
//!
//! Provides academic paper search (arxiv, Semantic Scholar, PubMed),
//! clinical trials search, PDF download + parse, and BibTeX generation.

pub mod arxiv;
pub mod bibtex;
pub mod clinical_trials;
pub mod memory;
pub mod pdf_fetch;
pub mod pubmed;
pub mod semantic_scholar;

pub use arxiv::ArxivSearchTool;
pub use bibtex::BibtexGenerateTool;
pub use clinical_trials::ClinicalTrialsSearchTool;
pub use memory::{ResearchRecallTool, ResearchRememberTool};
pub use pdf_fetch::PdfFetchTool;
pub use pubmed::PubMedSearchTool;
pub use semantic_scholar::SemanticScholarSearchTool;
