#!/usr/bin/env python3
"""
Paper Downloader for Collaborative Protocols Research (2026)
Downloads academic papers from multiple sources with fallback mechanisms.
"""

import os
import json
import time
import requests
import argparse
from pathlib import Path
from typing import Optional, Dict, List, Any
from dataclasses import dataclass, asdict
from datetime import datetime
import logging

# Third-party imports (with fallbacks)
try:
    import arxiv
    ARXIV_AVAILABLE = True
except ImportError:
    ARXIV_AVAILABLE = False
    print("Warning: arxiv library not available. Install with: pip install arxiv")

try:
    from scholarly import scholarly
    SCHOLARLY_AVAILABLE = True
except ImportError:
    SCHOLARLY_AVAILABLE = False
    print("Warning: scholarly library not available. Install with: pip install scholarly")

try:
    from crossref.restful import Works
    CROSSREF_AVAILABLE = True
except ImportError:
    CROSSREF_AVAILABLE = False
    print("Warning: crossrefapi library not available. Install with: pip install crossrefapi")

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

@dataclass
class PaperMetadata:
    """Metadata for a research paper."""
    title: str
    authors: List[str]
    abstract: str
    year: int
    doi: Optional[str] = None
    arxiv_id: Optional[str] = None
    url: Optional[str] = None
    pdf_url: Optional[str] = None
    source: str = "unknown"
    keywords: List[str] = None
    
    def __post_init__(self):
        if self.keywords is None:
            self.keywords = []
    
    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        return asdict(self)
    
    def to_json(self) -> str:
        """Convert to JSON string."""
        return json.dumps(self.to_dict(), indent=2, ensure_ascii=False)

class PaperDownloader:
    """Main class for downloading papers from multiple sources."""
    
    def __init__(self, output_dir: str = "papers"):
        """
        Initialize the downloader.
        
        Args:
            output_dir: Directory to save downloaded papers and metadata
        """
        self.output_dir = Path(output_dir)
        self.metadata_dir = self.output_dir / "metadata"
        self.pdf_dir = self.output_dir / "pdfs"
        
        # Create directories
        self.output_dir.mkdir(exist_ok=True)
        self.metadata_dir.mkdir(exist_ok=True)
        self.pdf_dir.mkdir(exist_ok=True)
        
        # User-Agent for requests
        self.headers = {
            'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36'
        }
        
        # Session for HTTP requests
        self.session = requests.Session()
        self.session.headers.update(self.headers)
        
        logger.info(f"Initialized PaperDownloader with output directory: {self.output_dir}")
    
    def search_arxiv(self, query: str, max_results: int = 10) -> List[PaperMetadata]:
        """Search for papers on arXiv."""
        if not ARXIV_AVAILABLE:
            logger.warning("arXiv library not available. Skipping arXiv search.")
            return []
        
        try:
            search = arxiv.Search(
                query=query,
                max_results=max_results,
                sort_by=arxiv.SortCriterion.Relevance
            )
            
            papers = []
            for result in search.results():
                paper = PaperMetadata(
                    title=result.title,
                    authors=[str(author) for author in result.authors],
                    abstract=result.summary,
                    year=result.published.year,
                    arxiv_id=result.entry_id.split('/')[-1],
                    url=result.entry_id,
                    pdf_url=result.pdf_url,
                    source="arxiv",
                    keywords=[cat for cat in result.categories]
                )
                papers.append(paper)
                logger.info(f"Found arXiv paper: {result.title}")
            
            return papers
            
        except Exception as e:
            logger.error(f"Error searching arXiv: {e}")
            return []
    
    def search_semantic_scholar(self, query: str, max_results: int = 10) -> List[PaperMetadata]:
        """Search for papers using Semantic Scholar API."""
        try:
            url = "https://api.semanticscholar.org/graph/v1/paper/search"
            params = {
                "query": query,
                "limit": max_results,
                "fields": "title,authors,abstract,year,doi,url,openAccessPdf"
            }
            
            response = self.session.get(url, params=params, timeout=30)
            response.raise_for_status()
            data = response.json()
            
            papers = []
            for item in data.get("data", []):
                authors = [author.get("name", "") for author in item.get("authors", [])]
                
                paper = PaperMetadata(
                    title=item.get("title", "Unknown Title"),
                    authors=authors,
                    abstract=item.get("abstract", ""),
                    year=item.get("year", 0),
                    doi=item.get("doi"),
                    url=item.get("url"),
                    pdf_url=item.get("openAccessPdf", {}).get("url") if item.get("openAccessPdf") else None,
                    source="semantic_scholar",
                    keywords=[]
                )
                papers.append(paper)
                logger.info(f"Found Semantic Scholar paper: {item.get('title', 'Unknown Title')}")
            
            return papers
            
        except Exception as e:
            logger.error(f"Error searching Semantic Scholar: {e}")
            return []
    
    def search_crossref(self, query: str, max_results: int = 10) -> List[PaperMetadata]:
        """Search for papers using CrossRef API."""
        if not CROSSREF_AVAILABLE:
            logger.warning("CrossRef library not available. Skipping CrossRef search.")
            return []
        
        try:
            works = Works()
            results = works.query(query).filter(has_abstract=True).sort("relevance")
            
            papers = []
            count = 0
            for work in results:
                if count >= max_results:
                    break
                
                title = work.get("title", ["Unknown Title"])[0]
                authors = [author.get("given", "") + " " + author.get("family", "") 
                          for author in work.get("author", [])]
                
                paper = PaperMetadata(
                    title=title,
                    authors=authors,
                    abstract=work.get("abstract", ""),
                    year=int(work.get("published", {}).get("date-parts", [[0]])[0][0]) if work.get("published") else 0,
                    doi=work.get("DOI"),
                    url=work.get("URL"),
                    pdf_url=None,  # CrossRef doesn't provide direct PDF URLs
                    source="crossref",
                    keywords=work.get("subject", [])
                )
                papers.append(paper)
                logger.info(f"Found CrossRef paper: {title}")
                count += 1
            
            return papers
            
        except Exception as e:
            logger.error(f"Error searching CrossRef: {e}")
            return []
    
    def download_pdf(self, paper: PaperMetadata) -> Optional[Path]:
        """Download PDF for a paper if available."""
        pdf_url = paper.pdf_url
        
        if not pdf_url:
            logger.warning(f"No PDF URL available for paper: {paper.title}")
            return None
        
        try:
            # Generate filename
            safe_title = "".join(c for c in paper.title if c.isalnum() or c in (' ', '-', '_')).rstrip()
            safe_title = safe_title[:100]  # Limit filename length
            filename = f"{safe_title}_{paper.year}.pdf"
            filepath = self.pdf_dir / filename
            
            # Download PDF
            response = self.session.get(pdf_url, timeout=60)
            response.raise_for_status()
            
            # Check if it's actually a PDF
            content_type = response.headers.get('content-type', '')
            if 'pdf' not in content_type.lower():
                logger.warning(f"URL {pdf_url} doesn't appear to be a PDF (content-type: {content_type})")
                return None
            
            # Save PDF
            with open(filepath, 'wb') as f:
                f.write(response.content)
            
            logger.info(f"Downloaded PDF: {filepath}")
            return filepath
            
        except Exception as e:
            logger.error(f"Error downloading PDF from {pdf_url}: {e}")
            return None
    
    def save_metadata(self, paper: PaperMetadata) -> Path:
        """Save paper metadata as JSON."""
        safe_title = "".join(c for c in paper.title if c.isalnum() or c in (' ', '-', '_')).rstrip()
        safe_title = safe_title[:100]  # Limit filename length
        filename = f"{safe_title}_{paper.year}.json"
        filepath = self.metadata_dir / filename
        
        with open(filepath, 'w', encoding='utf-8') as f:
            json.dump(paper.to_dict(), f, indent=2, ensure_ascii=False)
        
        logger.info(f"Saved metadata: {filepath}")
        return filepath
    
    def search_all_sources(self, query: str, max_per_source: int = 5) -> List[PaperMetadata]:
        """Search all available sources and deduplicate results."""
        all_papers = []
        
        # Search arXiv
        logger.info(f"Searching arXiv for: {query}")
        arxiv_papers = self.search_arxiv(query, max_per_source)
        all_papers.extend(arxiv_papers)
        
        # Search Semantic Scholar
        logger.info(f"Searching Semantic Scholar for: {query}")
        ss_papers = self.search_semantic_scholar(query, max_per_source)
        all_papers.extend(ss_papers)
        
        # Search CrossRef
        logger.info(f"Searching CrossRef for: {query}")
        crossref_papers = self.search_crossref(query, max_per_source)
        all_papers.extend(crossref_papers)
        
        # Deduplicate by DOI or title
        seen_dois = set()
        seen_titles = set()
        deduplicated = []
        
        for paper in all_papers:
            if paper.doi and paper.doi in seen_dois:
                continue
            if paper.title.lower() in seen_titles:
                continue
            
            if paper.doi:
                seen_dois.add(paper.doi)
            seen_titles.add(paper.title.lower())
            deduplicated.append(paper)
        
        logger.info(f"Found {len(deduplicated)} unique papers")
        return deduplicated
    
    def download_papers(self, papers: List[PaperMetadata], download_pdfs: bool = True) -> Dict[str, Any]:
        """Download papers and their PDFs."""
        results = {
            "total_papers": len(papers),
            "metadata_saved": 0,
            "pdfs_downloaded": 0,
            "failed_downloads": 0,
            "papers": []
        }
        
        for i, paper in enumerate(papers, 1):
            logger.info(f"Processing paper {i}/{len(papers)}: {paper.title}")
            
            # Save metadata
            metadata_path = self.save_metadata(paper)
            results["metadata_saved"] += 1
            
            # Download PDF if requested and available
            pdf_path = None
            if download_pdfs and paper.pdf_url:
                pdf_path = self.download_pdf(paper)
                if pdf_path:
                    results["pdfs_downloaded"] += 1
                else:
                    results["failed_downloads"] += 1
            
            # Add to results
            paper_result = {
                "title": paper.title,
                "metadata_path": str(metadata_path),
                "pdf_path": str(pdf_path) if pdf_path else None,
                "source": paper.source,
                "has_pdf": pdf_path is not None
            }
            results["papers"].append(paper_result)
            
            # Be respectful to APIs
            time.sleep(1)
        
        return results

def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Download academic papers on collaborative protocols")
    parser.add_argument("--query", type=str, default="collaborative protocols 2026 distributed systems",
                       help="Search query for papers")
    parser.add_argument("--output", type=str, default="papers",
                       help="Output directory for downloaded papers")
    parser.add_argument("--max-papers", type=int, default=10,
                       help="Maximum number of papers to download per source")
    parser.add_argument("--skip-pdfs", action="store_true",
                       help="Skip PDF downloads, only save metadata")
    parser.add_argument("--list-sources", action="store_true",
                       help="List available paper sources")
    
    args = parser.parse_args()
    
    if args.list_sources:
        print("Available paper sources:")
        print("1. arXiv (preprints in physics, mathematics, computer science, etc.)")
        print("2. Semantic Scholar (academic papers with open access PDFs)")
        print("3. CrossRef (scholarly publishing metadata)")
        print("\nNote: Some sources require internet access and may have rate limits.")
        return
    
    # Initialize downloader
    downloader = PaperDownloader(args.output)
    
    # Search for papers
    print(f"Searching for papers with query: '{args.query}'")
    papers = downloader.search_all_sources(args.query, args.max_papers)
    
    if not papers:
        print("No papers found. Try a different search query.")
        return
    
    # Display found papers
    print(f"\nFound {len(papers)} papers:")
    for i, paper in enumerate(papers, 1):
        print(f"{i}. {paper.title} ({paper.year}) - {paper.source}")
        if paper.authors:
            print(f"   Authors: {', '.join(paper.authors[:3])}{'...' if len(paper.authors) > 3 else ''}")
        print()
    
    # Ask for confirmation
    response = input(f"Do you want to download {len(papers)} papers? (y/n): ")
    if response.lower() != 'y':
        print("Download cancelled.")
        return
    
    # Download papers
    print("\nStarting download...")
    results = downloader.download_papers(papers, not args.skip_pdfs)
    
    # Print summary
    print("\n" + "="*50)
    print("DOWNLOAD SUMMARY")
    print("="*50)
    print(f"Total papers processed: {results['total_papers']}")
    print(f"Metadata files saved: {results['metadata_saved']}")
    print(f"PDFs downloaded: {results['pdfs_downloaded']}")
    print(f"Failed PDF downloads: {results['failed_downloads']}")
    print(f"\nOutput directory: {args.output}")
    print(f"  - Metadata: {args.output}/metadata/")
    print(f"  - PDFs: {args.output}/pdfs/")
    
    # Save summary report
    summary_path = Path(args.output) / "download_summary.json"
    with open(summary_path, 'w') as f:
        json.dump(results, f, indent=2)
    
    print(f"\nSummary saved to: {summary_path}")
    print("\nDone!")

if __name__ == "__main__":
    main()