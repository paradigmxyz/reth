from playwright.sync_api import sync_playwright
import time

# The pinned thread URL - switch to call tree view
BASE_URL = "https://profiler.firefox.com/public/q0h10v5sjtqstm2sj90azpzxpmhjd8r3agnx058"
CALL_TREE_URL = BASE_URL + "/calltree/?globalTrackOrder=201&hiddenGlobalTracks=01&hiddenLocalTracksByPid=1153071.2-0wh~1153071.1-xgwxiz7z9wA5A7wAp&localTrackOrderByPid=1153071.1-xhxiz7z9wA5A7wApz8xjwz1Di0wxfz2wz6A6AqAswDfArDgDhxg&range=19841m4475&symbolServer=http%3A%2F%2F127.0.0.1%3A3001%2F24w2lg79pyph7f2pbcpqnhsfk1rrhp6yl4pr074&thread=E5&v=12"

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    page = browser.new_page()
    
    print(f"Loading: {CALL_TREE_URL}")
    page.goto(CALL_TREE_URL, timeout=60000)
    page.wait_for_load_state('networkidle')
    time.sleep(8)  # Extra time for profile to load
    
    page.screenshot(path='/tmp/pinned_thread_calltree.png', full_page=True)
    print("Screenshot saved to /tmp/pinned_thread_calltree.png")
    
    # Get the full page text
    body_text = page.locator('body').inner_text()
    print("\n" + "="*80)
    print("FULL PAGE TEXT:")
    print("="*80)
    print(body_text[:8000])
    
    browser.close()
