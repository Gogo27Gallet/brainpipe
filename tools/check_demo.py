"""Browser smoke test for an already-running local API (browser-test extra)."""

import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8000")
    parser.add_argument("--chromium", help="Optional system Chromium executable")
    parser.add_argument("--screenshot", type=Path)
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright

    with sync_playwright() as p:
        browser = p.chromium.launch(executable_path=args.chromium, headless=True)
        page = browser.new_page(viewport={"width": 1440, "height": 1050})
        errors = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.goto(args.url.rstrip("/") + "/demo")
        document = "# Guide BrainPipe\n\n## Installation\n\nBonjour 世界 🧠. Une extraction locale et traçable.\n\n| Étape | Statut |\n|---|---|\n| Extraction | Prête |\n\n```python\nprint('hello')\n```\n"
        page.locator("#file").set_input_files(
            {
                "name": "guide.md",
                "mimeType": "text/markdown",
                "buffer": document.encode(),
            }
        )
        page.locator("#run").click()
        page.locator("#results").wait_for(state="visible", timeout=30_000)
        assert page.locator("#chunk-count").inner_text() == "5"
        page.locator("#chunks-tab").click()
        page.locator(".chunk").nth(2).click()
        assert page.locator("mark").count() == 1
        if args.screenshot:
            page.screenshot(path=str(args.screenshot), full_page=True)
        page.set_viewport_size({"width": 390, "height": 844})
        assert page.evaluate(
            "document.documentElement.scrollWidth <= window.innerWidth"
        )
        assert not errors, errors
        browser.close()
        print(
            "Desktop/mobile upload, polling, highlighting and overflow checks passed."
        )


if __name__ == "__main__":
    main()
