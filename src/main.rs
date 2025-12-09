use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use colored::*;
use reqwest::Client;
use scraper::{Html, Selector};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BASE_URL: &str = "https://news.ycombinator.com";
const ITEMS_PER_PAGE: usize = 30;
const CACHE_TTL_SECONDS: u64 = 300; // 5 minutes

// Safe selector init
macro_rules! safe_selector {
    ($name:ident, $pattern:expr) => {
        fn $name() -> &'static Selector {
            static CELL: OnceLock<Selector> = OnceLock::new();
            CELL.get_or_init(|| {
                Selector::parse($pattern)
                    .unwrap_or_else(|_| panic!("Invalid CSS selector: {}", $pattern))
            })
        }
    };
}

safe_selector!(row_selector, "tr.athing");
safe_selector!(subtext_selector, "tr > td.subtext");
safe_selector!(title_selector, "span.titleline > a");
safe_selector!(rank_selector, "span.rank");
safe_selector!(score_selector, "span.score");
safe_selector!(age_selector, "span.age a");
safe_selector!(user_selector, "a.hnuser");
safe_selector!(comment_link_selector, "a");
safe_selector!(title_display_selector, "span.titleline");
safe_selector!(text_selector, "div.toptext");
safe_selector!(comment_selector, "tr.athing.comtr");

#[derive(Parser)]
#[command(name = "hn")]
#[command(about = "A modern HackerNews CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List top stories (default)
    #[command(alias = "t")]
    Top {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// List new stories
    #[command(alias = "n")]
    New {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// List best stories
    #[command(alias = "b")]
    Best {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// List Ask HN stories
    #[command(alias = "a")]
    Ask {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// List Show HN stories
    #[command(alias = "s")]
    Show {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// List Job stories
    #[command(alias = "j")]
    Job {
        #[arg(short, long, default_value_t = 1)]
        page: usize,
    },
    /// Show story details and comments by rank from cache
    #[command(alias = "d")]
    Details {
        #[arg(help = "Story rank from the list or item ID")]
        id_or_rank: String,
    },
    /// Open story in browser
    #[command(alias = "o")]
    Open { index: usize },
    /// Show user details
    #[command(alias = "u")]
    User { username: String },
    /// Fetch multiple pages at once
    #[command(alias = "m")]
    Multi {
        #[arg(short, long, default_value = "top")]
        category: String,
        #[arg(short, long, default_value = "3")]
        num_pages: usize,
    },
}

#[derive(Debug, Clone)]
struct Story {
    rank: usize,
    id: String,
    title: String,
    url: Option<String>,
    points: Option<usize>,
    author: Option<String>,
    comments: Option<usize>,
    age: Option<String>,
}

impl Story {
    fn to_cache_line(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.rank,
            self.id,
            self.title.replace('|', "∣"),
            self.url.as_ref().map(|s| s.as_str()).unwrap_or(""),
            self.points.map(|p| p.to_string()).unwrap_or_default(),
            self.author.as_ref().map(|s| s.as_str()).unwrap_or(""),
            self.comments.map(|c| c.to_string()).unwrap_or_default()
        )
    }

    fn from_cache_line(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 7 {
            return None;
        }

        Some(Story {
            rank: parts[0].parse().ok()?,
            id: parts[1].to_string(),
            title: parts[2].replace('∣', "|"),
            url: if parts[3].is_empty() {
                None
            } else {
                Some(parts[3].to_string())
            },
            points: parts[4].parse().ok(),
            author: if parts[5].is_empty() {
                None
            } else {
                Some(parts[5].to_string())
            },
            comments: parts[6].parse().ok(),
            age: None,
        })
    }
}

struct HnScraper {
    client: Client,
}

impl HnScraper {
    fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
            .pool_max_idle_per_host(10)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client })
    }

    async fn fetch_stories(&self, endpoint: &str, page: usize) -> Result<Vec<Story>> {
        let url = if page > 1 {
            format!("{}/{}?p={}", BASE_URL, endpoint, page)
        } else {
            format!("{}/{}", BASE_URL, endpoint)
        };

        let html = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send HTTP request")?
            .text()
            .await
            .context("Failed to read response text")?;

        let document = Html::parse_document(&html);
        let mut stories = Vec::with_capacity(ITEMS_PER_PAGE);

        let story_rows: Vec<_> = document.select(row_selector()).collect();
        let subtext_rows: Vec<_> = document.select(subtext_selector()).collect();

        for (idx, row) in story_rows.iter().enumerate() {
            let id = row.value().attr("id").unwrap_or("unknown").to_string();

            let rank = row
                .select(rank_selector())
                .next()
                .and_then(|r| r.inner_html().trim_end_matches('.').parse().ok())
                .unwrap_or((page - 1) * ITEMS_PER_PAGE + idx + 1);

            let title_elem = row.select(title_selector()).next();
            let title = title_elem.map(|e| e.inner_html()).unwrap_or_default();

            if title.is_empty() {
                continue;
            }

            let url = title_elem.and_then(|e| e.value().attr("href")).map(|s| {
                if s.starts_with("http") {
                    s.to_string()
                } else {
                    format!("{}/{}", BASE_URL, s.trim_start_matches('/'))
                }
            });

            let mut points = None;
            let mut author = None;
            let mut comments = None;
            let mut age = None;

            if let Some(subtext) = subtext_rows.get(idx) {
                if let Some(score) = subtext.select(score_selector()).next() {
                    points = score
                        .inner_html()
                        .split_whitespace()
                        .next()
                        .and_then(|s| s.parse().ok());
                }

                if let Some(user) = subtext.select(user_selector()).next() {
                    author = Some(user.inner_html());
                }

                if let Some(age_elem) = subtext.select(age_selector()).next() {
                    age = Some(age_elem.inner_html());
                }

                for link in subtext.select(comment_link_selector()) {
                    let text = link.inner_html();
                    if text.contains("comment") || text.contains("discuss") {
                        comments = text
                            .split_whitespace()
                            .next()
                            .and_then(|s| s.replace("&nbsp;", "").parse().ok());
                        break;
                    }
                }
            }

            stories.push(Story {
                rank,
                id,
                title,
                url,
                points,
                author,
                comments,
                age,
            });
        }

        if stories.is_empty() {
            bail!(
                "No stories found on page {}. The page structure may have changed.",
                page
            );
        }

        Ok(stories)
    }

    async fn fetch_item(&self, id: &str) -> Result<()> {
        let url = format!("{}/item?id={}", BASE_URL, id);
        let html = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch item")?
            .text()
            .await
            .context("Failed to read item response")?;

        let document = Html::parse_document(&html);

        // Get title and URL
        if let Some(title_elem) = document.select(title_display_selector()).next() {
            // Create one-time selector for link
            let link_sel = Selector::parse("a")
                .map_err(|e| anyhow::anyhow!("Failed to parse link selector: {:?}", e))?;

            if let Some(link) = title_elem.select(&link_sel).next() {
                let title = link.inner_html();
                let story_url = link.value().attr("href").map(|u| {
                    if u.starts_with("http") {
                        u.to_string()
                    } else {
                        format!("{}/{}", BASE_URL, u.trim_start_matches('/'))
                    }
                });

                if let Some(story_url) = story_url {
                    println!("{}", title.bright_white().bold());
                    println!(
                        "{} {}\n",
                        "Link:".bright_cyan(),
                        ansi_link(&story_url, &story_url)
                    );
                } else {
                    println!("{}\n", title.bright_white().bold());
                }
            }
        }

        // Get story text if available
        if let Some(text_elem) = document.select(text_selector()).next() {
            let text = text_elem.text().collect::<String>();
            if !text.trim().is_empty() {
                println!("{}\n", text.trim());
            }
        }

        // Display comments
        let comment_count = document.select(comment_selector()).count();

        if comment_count == 0 {
            println!("{}", "No comments yet".bright_black());
            return Ok(());
        }

        println!(
            "{} {}\n",
            "Comments:".bright_cyan().bold(),
            format!("({} total)", comment_count).bright_black()
        );

        // Create selectors for comment parsing
        let comhead_selector = Selector::parse("span.comhead")
            .map_err(|e| anyhow::anyhow!("Failed to parse comhead selector: {:?}", e))?;
        let commtext_selector = Selector::parse("div.commtext")
            .map_err(|e| anyhow::anyhow!("Failed to parse commtext selector: {:?}", e))?;
        let ind_selector = Selector::parse("td.ind")
            .map_err(|e| anyhow::anyhow!("Failed to parse ind selector: {:?}", e))?;
        let author_selector = Selector::parse("a.hnuser")
            .map_err(|e| anyhow::anyhow!("Failed to parse author selector: {:?}", e))?;
        let age_comment_selector = Selector::parse("span.age a")
            .map_err(|e| anyhow::anyhow!("Failed to parse age selector: {:?}", e))?;

        for (idx, comment_row) in document.select(comment_selector()).enumerate() {
            if idx >= 10 {
                println!(
                    "\n{}",
                    format!("... {} more comments", comment_count - 10).bright_black()
                );
                break;
            }

            let indent_level = comment_row
                .select(&ind_selector)
                .next()
                .and_then(|td| td.value().attr("indent"))
                .and_then(|i| i.parse::<usize>().ok())
                .unwrap_or(0);

            let indent = "  ".repeat(indent_level);

            if let Some(comhead) = comment_row.select(&comhead_selector).next() {
                let author = comhead
                    .select(&author_selector)
                    .next()
                    .map(|a| a.inner_html())
                    .unwrap_or_else(|| "[deleted]".to_string());

                let age = comhead
                    .select(&age_comment_selector)
                    .next()
                    .map(|a| a.inner_html())
                    .unwrap_or_default();

                println!(
                    "{}{} {} {}",
                    indent,
                    "●".bright_black(),
                    author.cyan(),
                    age.bright_black()
                );
            }

            if let Some(commtext) = comment_row.select(&commtext_selector).next() {
                let text = commtext.text().collect::<Vec<_>>().join(" ");
                let cleaned_text = text
                    .trim()
                    .lines()
                    .map(|line| line.trim())
                    .filter(|line| !line.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");

                let wrapped = wrap_text(&cleaned_text, 80 - (indent_level * 2 + 2));
                for line in wrapped {
                    println!("{}  {}", indent, line);
                }
            }

            println!();
        }

        Ok(())
    }

    async fn fetch_user(&self, username: &str) -> Result<()> {
        let url = format!("{}/user?id={}", BASE_URL, username);
        let html = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch user")?
            .text()
            .await
            .context("Failed to read user response")?;

        let document = Html::parse_document(&html);

        println!(
            "{} {}\n",
            "Profile:".bright_cyan().bold(),
            username.bright_white()
        );

        let tr_selector = Selector::parse("tr")
            .map_err(|e| anyhow::anyhow!("Failed to parse tr selector: {:?}", e))?;
        let td_selector = Selector::parse("td")
            .map_err(|e| anyhow::anyhow!("Failed to parse td selector: {:?}", e))?;

        let mut found_data = false;

        for row in document.select(&tr_selector) {
            let cells: Vec<_> = row.select(&td_selector).collect();

            if cells.len() == 2 {
                let field = cells[0].text().collect::<String>().trim().to_string();
                let value_text = cells[1].text().collect::<String>().trim().to_string();

                if field.ends_with(':') {
                    let field_name = field.trim_end_matches(':');

                    match field_name {
                        "user" => {
                            println!(
                                "{}: {}",
                                "Username".bright_yellow(),
                                value_text.bright_white()
                            );
                            found_data = true;
                        }
                        "created" => {
                            println!(
                                "{}: {}",
                                "Created".bright_yellow(),
                                value_text.bright_white()
                            );
                            found_data = true;
                        }
                        "karma" => {
                            println!("{}: {}", "Karma".bright_yellow(), value_text.bright_white());
                            found_data = true;
                        }
                        "about" => {
                            let about_html = cells[1].inner_html().trim().to_string();
                            println!("{}: {}", "About".bright_yellow(), about_html.bright_white());
                            found_data = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        if !found_data {
            bail!("User '{}' not found or has no public information", username);
        }

        println!();
        Ok(())
    }

    async fn fetch_multiple_pages(
        &self,
        endpoint: &str,
        pages: Vec<usize>,
    ) -> Result<Vec<Vec<Story>>> {
        let futures = pages
            .into_iter()
            .map(|page| self.fetch_stories(endpoint, page));
        let results = futures::future::join_all(futures).await;
        results.into_iter().collect()
    }
}

fn get_cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("hn-cli")
        .join("stories.cache")
}

fn save_stories(stories: &[Story]) -> Result<()> {
    let cache_path = get_cache_path();
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent).context("Failed to create cache directory")?;
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Failed to get system time")?
        .as_secs();

    let mut cache_content = format!("{}\n", timestamp);
    cache_content.push_str(
        &stories
            .iter()
            .map(|s| s.to_cache_line())
            .collect::<Vec<_>>()
            .join("\n"),
    );

    fs::write(&cache_path, cache_content).context("Failed to write cache file")?;
    Ok(())
}

fn load_cached_stories() -> Result<Vec<Story>> {
    let cache_path = get_cache_path();
    let content = fs::read_to_string(&cache_path).context("Failed to read cache file")?;

    let mut lines = content.lines();

    if let Some(timestamp_str) = lines.next() {
        if let Ok(timestamp) = timestamp_str.parse::<u64>() {
            let current_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("Failed to get current time")?
                .as_secs();

            if current_time - timestamp > CACHE_TTL_SECONDS {
                bail!("Cache expired");
            }
        }
    }

    let stories: Vec<Story> = lines.filter_map(Story::from_cache_line).collect();

    if stories.is_empty() {
        bail!("No stories in cache");
    }

    Ok(stories)
}

fn display_stories(stories: &[Story]) {
    for story in stories {
        println!(
            "{}. {} {}",
            story.rank.to_string().bright_black(),
            story.title.bright_white().bold(),
            story
                .url
                .as_ref()
                .map(|u| format!("({})", extract_domain(u))
                    .bright_black()
                    .to_string())
                .unwrap_or_default()
        );

        let mut meta = Vec::new();
        if let Some(points) = story.points {
            meta.push(format!("{} points", points).yellow().to_string());
        }
        if let Some(author) = &story.author {
            meta.push(format!("by {}", author).cyan().to_string());
        }
        if let Some(age) = &story.age {
            meta.push(age.bright_black().to_string());
        }
        if let Some(comments) = story.comments {
            meta.push(format!("{} comments", comments).green().to_string());
        }

        if !meta.is_empty() {
            println!("   {}", meta.join(" | "));
        }
        println!();
    }
}

fn extract_domain(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|s| s.split('/').next())
        .unwrap_or(url)
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.len() + word.len() + 1 > width {
            if !current_line.is_empty() {
                lines.push(current_line.clone());
                current_line.clear();
            }
        }

        if !current_line.is_empty() {
            current_line.push(' ');
        }
        current_line.push_str(word);
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
}

fn ansi_link(url: &str, text: &str) -> String {
    format!(
        "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
        url,
        text.cyan().underline()
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let scraper = HnScraper::new().context("Failed to initialize scraper")?;

    match cli.command.unwrap_or(Commands::Top { page: 1 }) {
        Commands::Top { page } => {
            let stories = scraper
                .fetch_stories("news", page)
                .await
                .context("Failed to fetch top stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::New { page } => {
            let stories = scraper
                .fetch_stories("newest", page)
                .await
                .context("Failed to fetch new stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::Best { page } => {
            let stories = scraper
                .fetch_stories("best", page)
                .await
                .context("Failed to fetch best stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::Ask { page } => {
            let stories = scraper
                .fetch_stories("ask", page)
                .await
                .context("Failed to fetch Ask HN stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::Show { page } => {
            let stories = scraper
                .fetch_stories("show", page)
                .await
                .context("Failed to fetch Show HN stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::Job { page } => {
            let stories = scraper
                .fetch_stories("jobs", page)
                .await
                .context("Failed to fetch Job stories")?;
            save_stories(&stories)?;
            display_stories(&stories);
        }
        Commands::Details { id_or_rank } => {
            if let Ok(rank) = id_or_rank.parse::<usize>() {
                match load_cached_stories() {
                    Ok(stories) => {
                        if let Some(story) = stories.iter().find(|s| s.rank == rank) {
                            scraper
                                .fetch_item(&story.id)
                                .await
                                .context("Failed to fetch item details")?;
                        } else {
                            bail!(
                                "Story with rank {} not found in cache. Run a list command first.",
                                rank
                            );
                        }
                    }
                    Err(_) => {
                        bail!(
                            "No cached stories. Please run a list command (top, new, etc.) first."
                        );
                    }
                }
            } else {
                scraper
                    .fetch_item(&id_or_rank)
                    .await
                    .context("Failed to fetch item details")?;
            }
        }
        Commands::Open { index } => {
            let stories = load_cached_stories()
                .context("Failed to load cached stories. Run a command first to populate cache.")?;

            if let Some(story) = stories.iter().find(|s| s.rank == index) {
                if let Some(url) = &story.url {
                    open::that(url).context("Failed to open URL in browser")?;
                    println!("{} {}", "Opened:".green(), url);
                } else {
                    let hn_url = format!("{}/item?id={}", BASE_URL, story.id);
                    open::that(&hn_url).context("Failed to open HN URL in browser")?;
                    println!("{} {}", "Opened HN discussion:".green(), hn_url);
                }
            } else {
                bail!("Story with rank {} not found in cache", index);
            }
        }
        Commands::User { username } => {
            scraper
                .fetch_user(&username)
                .await
                .context(format!("Failed to fetch user: {}", username))?;
        }
        Commands::Multi {
            category,
            num_pages,
        } => {
            let endpoint = match category.as_str() {
                "top" => "news",
                "new" => "newest",
                "best" => "best",
                "ask" => "ask",
                "show" => "show",
                "job" => "jobs",
                _ => "news",
            };

            let pages: Vec<usize> = (1..=num_pages).collect();
            let all_stories = scraper
                .fetch_multiple_pages(endpoint, pages)
                .await
                .context("Failed to fetch multiple pages")?;

            let flattened: Vec<Story> = all_stories.into_iter().flatten().collect();
            save_stories(&flattened)?;
            display_stories(&flattened);

            println!(
                "\n{} Fetched {} stories from {} pages in parallel",
                "✓".green(),
                flattened.len().to_string().bright_white().bold(),
                num_pages.to_string().bright_white().bold()
            );
        }
    }

    Ok(())
}
