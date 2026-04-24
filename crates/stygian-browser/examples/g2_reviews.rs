//! Example: Extract G2 reviews from the hydrated DOM
//!
//! This example uses the browser workflow rather than a direct HTTP fetch.
//! It navigates to a G2 review page, waits for the Turbo Frame review section
//! to hydrate, expands visible "Show More" panels, performs a few bounded
//! scroll passes, and emits structured JSON for the currently loaded review
//! page.
//!
//! ```sh
//! cargo run --example g2_reviews -p stygian-browser
//! cargo run --example g2_reviews -p stygian-browser -- https://www.g2.com/products/jira/reviews?page=2
//! cargo run --example g2_reviews -p stygian-browser -- https://www.g2.com/products/jira/reviews | jq .
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use stygian_browser::config::{PoolConfig, StealthLevel};
use stygian_browser::{BrowserConfig, BrowserPool, WaitUntil};
use tokio::time::sleep;

const DEFAULT_G2_URL: &str = "https://www.g2.com/products/jira/reviews";
const REVIEWS_FRAME_SELECTOR: &str = "turbo-frame#reviews-and-filters";
const REVIEW_BODY_SELECTOR: &str = "turbo-frame#reviews-and-filters [itemprop=\"reviewBody\"]";
const EXPAND_SHOW_MORE_JS: &str = r"(() => {
    const frame = document.querySelector('turbo-frame#reviews-and-filters');
    if (!frame) {
        return 0;
    }

    const buttons = Array.from(frame.querySelectorAll('button')).filter((button) => {
        const label = button.textContent?.replace(/\s+/g, ' ').trim() ?? '';
        return label.includes('Show More') && !button.disabled;
    });

    for (const button of buttons.slice(0, 12)) {
        button.click();
    }

    return buttons.length;
})()";
const EXTRACT_REVIEWS_JS: &str = r#"(() => {
    const frame = document.querySelector('turbo-frame#reviews-and-filters');
    if (!frame) {
        return {
            frame_found: false,
            frame_heading: null,
            sort_options: [],
            current_page: null,
            pagination: [],
            reviews: [],
        };
    }

    const normalizeWhitespace = (value) =>
        (value ?? '').replace(/\s+/g, ' ').trim();

    const normalizeKey = (value) =>
        normalizeWhitespace(value)
            .toLowerCase()
            .replace(/[^a-z0-9]+/g, '_')
            .replace(/^_+|_+$/g, '');

    const pickText = (root, selectors) => {
        for (const selector of selectors) {
            const node = root.querySelector(selector);
            const text = normalizeWhitespace(node?.textContent);
            if (text) {
                return text;
            }
        }
        return null;
    };

    const findRating = (body) => {
        let current = body;
        for (let depth = 0; current && depth < 6; depth += 1) {
            const label = Array.from(current.querySelectorAll('label, span, div'))
                .map((node) => normalizeWhitespace(node.textContent))
                .find((text) => /^\d(?:\.\d)?\/5$/.test(text));
            if (label) {
                return label;
            }
            current = current.parentElement;
        }
        return null;
    };

    const reviewBodies = Array.from(frame.querySelectorAll('[itemprop="reviewBody"]'));
    const reviews = reviewBodies.map((body, index) => {
        const container =
            body.closest('article, li, [class*="review"], [class*="Review"], [class*="paper"]')
            || body.parentElement
            || body;

        const sections = Object.fromEntries(
            Array.from(body.querySelectorAll('section')).map((section) => {
                const heading = normalizeWhitespace(section.querySelector('div')?.textContent);
                const paragraphs = Array.from(section.querySelectorAll('p'))
                    .map((node) => normalizeWhitespace(node.textContent))
                    .filter(Boolean)
                    .map((text) => text.replace(/Review collected by and hosted on G2\.com\.?/g, '').trim())
                    .filter(Boolean);
                return [normalizeKey(heading), {
                    heading,
                    text: paragraphs.join('\n\n') || null,
                }];
            }).filter(([key, value]) => key && value.text)
        );

        return {
            index: index + 1,
            rating: findRating(body),
            reviewer: pickText(container, [
                'a[href*="/users/"]',
                '[itemprop="author"] a',
                '[itemprop="author"]',
            ]),
            date: pickText(container, ['time', '[datetime]']),
            sections,
        };
    });

    const pagination = Array.from(frame.querySelectorAll('a.pagination__page-number-link, a.pagination__named-link')).map((link) => ({
        text: normalizeWhitespace(link.textContent),
        href: link.href || link.getAttribute('href'),
    }));

    const currentPage = normalizeWhitespace(
        frame.querySelector('.pagination__page-number--current')?.textContent
    );

    const sortOptions = Array.from(
        frame.querySelectorAll('[data-elv--form--selectbox-controller-choices-value]')
    ).flatMap((node) => {
        try {
            return JSON.parse(node.getAttribute('data-elv--form--selectbox-controller-choices-value') || '[]');
        } catch {
            return [];
        }
    });

    return {
        frame_found: true,
        frame_heading: normalizeWhitespace(frame.querySelector('h3')?.textContent),
        sort_options: sortOptions,
        current_page: currentPage || null,
        pagination,
        review_count_on_page: reviews.length,
        reviews,
    };
})()"#;

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

async fn emit_missing_frame_result(
    page: &stygian_browser::PageHandle,
    url: &str,
    wait_stage: &str,
    wait_error: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let final_url = page.url().await.unwrap_or_else(|_| url.to_string());
    let title = page.title().await.unwrap_or_default();
    let status_code = page.status_code().unwrap_or(None).unwrap_or(0);
    let text_excerpt: String = page
        .eval("(document.body?.innerText || '').trim().replace(/\\s+/g, ' ').slice(0, 800)")
        .await
        .unwrap_or_default();
    let html_excerpt = page
        .content()
        .await
        .map(|html| html.chars().take(800).collect::<String>())
        .unwrap_or_default();

    let result = json!({
        "url": url,
        "final_url": final_url,
        "status_code": status_code,
        "title": title,
        "extracted_at": epoch_secs(),
        "extraction": {
            "frame_found": false,
            "wait_stage": wait_stage,
            "wait_error": wait_error,
            "text_excerpt": text_excerpt,
            "html_excerpt": html_excerpt,
        },
    });

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn wait_for_reviews_or_emit(
    page: &stygian_browser::PageHandle,
    url: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    if let Err(error) = page
        .wait_for_selector(REVIEWS_FRAME_SELECTOR, Duration::from_secs(45))
        .await
    {
        emit_missing_frame_result(page, url, "reviews_frame", &error.to_string()).await?;
        return Ok(false);
    }

    if let Err(error) = page
        .wait_for_selector(REVIEW_BODY_SELECTOR, Duration::from_secs(45))
        .await
    {
        emit_missing_frame_result(page, url, "review_bodies", &error.to_string()).await?;
        return Ok(false);
    }

    Ok(true)
}

async fn expand_show_more_sections(page: &stygian_browser::PageHandle, phase: &str) -> u64 {
    let expanded: u64 = page.eval(EXPAND_SHOW_MORE_JS).await.unwrap_or(0);
    if expanded > 0 {
        eprintln!("[g2] expanded {expanded} collapsed sections {phase}");
        sleep(Duration::from_millis(1200)).await;
    }
    expanded
}

async fn perform_scroll_passes(page: &stygian_browser::PageHandle) {
    for pass in 1..=4 {
        let offset: i64 = page
            .eval("Math.max(600, Math.round(window.innerHeight * 0.85))")
            .await
            .unwrap_or(800);

        eprintln!("[g2] scroll pass {pass}/4 ({offset}px)");
        let script = format!("window.scrollBy({{ top: {offset}, behavior: 'smooth' }});");
        page.eval::<Value>(&script).await.ok();
        sleep(Duration::from_millis(1200)).await;
    }
}

async fn extract_reviews(page: &stygian_browser::PageHandle) -> Value {
    page.eval(EXTRACT_REVIEWS_JS).await.unwrap_or_else(|_| {
        json!({
            "frame_found": false,
            "frame_heading": null,
            "sort_options": [],
            "current_page": null,
            "pagination": [],
            "review_count_on_page": 0,
            "reviews": [],
        })
    })
}

async fn build_result_payload(
    page: &stygian_browser::PageHandle,
    url: &str,
    expanded_before_scroll: u64,
    expanded_after_scroll: u64,
) -> Value {
    let final_url = page.url().await.unwrap_or_else(|_| url.to_string());
    let title = page.title().await.unwrap_or_default();
    let status_code = page.status_code().unwrap_or(None).unwrap_or(0);
    let extracted = extract_reviews(page).await;

    json!({
        "url": url,
        "final_url": final_url,
        "status_code": status_code,
        "title": title,
        "expanded_before_scroll": expanded_before_scroll,
        "expanded_after_scroll": expanded_after_scroll,
        "extracted_at": epoch_secs(),
        "extraction": extracted,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_G2_URL.to_string());

    eprintln!("[g2] target : {url}");

    let config = BrowserConfig::builder()
        .headless(true)
        .stealth_level(StealthLevel::Advanced)
        .pool(PoolConfig {
            min_size: 1,
            max_size: 2,
            idle_timeout: Duration::from_mins(1),
            acquire_timeout: Duration::from_secs(30),
        })
        .build();

    eprintln!("[g2] warming browser pool...");
    let pool = BrowserPool::new(config).await?;
    let handle = pool.acquire().await?;
    let browser = handle
        .browser()
        .ok_or("browser pool returned an expired handle")?;
    let mut page = browser.new_page().await?;

    eprintln!("[g2] navigating to review page...");
    page.navigate(&url, WaitUntil::DomContentLoaded, Duration::from_secs(75))
        .await?;

    if !wait_for_reviews_or_emit(&page, &url).await? {
        page.close().await.ok();
        handle.release().await;
        eprintln!("[g2] review content unavailable; emitted diagnostic payload.");
        return Ok(());
    }

    sleep(Duration::from_millis(1500)).await;
    let expanded_before_scroll = expand_show_more_sections(&page, "before scrolling").await;
    perform_scroll_passes(&page).await;
    let expanded_after_scroll = expand_show_more_sections(&page, "after scrolling").await;

    let result =
        build_result_payload(&page, &url, expanded_before_scroll, expanded_after_scroll).await;

    println!("{}", serde_json::to_string_pretty(&result)?);

    page.close().await.ok();
    handle.release().await;

    eprintln!("[g2] done.");
    Ok(())
}
