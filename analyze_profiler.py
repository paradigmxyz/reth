from playwright.sync_api import sync_playwright
import time

URLS = {
    "slow_block_main": "https://share.firefox.dev/45a9S1G",
    "normal_blocks": "https://share.firefox.dev/4j7tadO",
    "pinned_thread": "https://share.firefox.dev/3LylcxY",
    "proof_fetch_tasks": "https://share.firefox.dev/4pAZyac",
}

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    
    for name, url in URLS.items():
        print(f"\n{'='*60}")
        print(f"PROFILE: {name}")
        print(f"URL: {url}")
        print('='*60)
        
        page = browser.new_page()
        page.goto(url, timeout=60000)
        
        # Wait for the profiler to fully load
        page.wait_for_load_state('networkidle')
        time.sleep(5)  # Extra time for JS rendering
        
        # Get the final URL (profiler redirects)
        final_url = page.url
        print(f"Final URL: {final_url}")
        
        # Take a screenshot
        page.screenshot(path=f'/tmp/{name}.png', full_page=True)
        print(f"Screenshot saved to /tmp/{name}.png")
        
        # Try to extract call tree content
        try:
            # Look for the call tree table
            call_tree = page.locator('.treeView, .call-tree, [class*="CallTree"], [class*="callTree"]').first
            if call_tree:
                text = call_tree.inner_text()
                print(f"\nCall Tree Content (first 3000 chars):\n{text[:3000]}")
        except Exception as e:
            print(f"Could not extract call tree: {e}")
        
        # Get visible text content
        try:
            body_text = page.locator('body').inner_text()
            # Filter for relevant content
            lines = body_text.split('\n')
            relevant = [l for l in lines if any(kw in l.lower() for kw in 
                ['%', 'reth', 'proof', 'trie', 'hash', 'fetch', 'state', 'root', 
                 'rayon', 'tokio', 'engine', 'execute', 'sload', 'journal'])]
            print(f"\nRelevant lines:\n" + '\n'.join(relevant[:100]))
        except Exception as e:
            print(f"Could not extract body: {e}")
        
        page.close()
    
    browser.close()
