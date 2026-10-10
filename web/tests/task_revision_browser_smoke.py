"""Local, disposable browser E2E for Task Revision WebUI at localhost:18986."""
import json
from urllib.request import Request, urlopen
from playwright.sync_api import sync_playwright

base="http://127.0.0.1:18986"
def create_fixture():
    body=json.dumps({"title":"Revision browser fixture",
      "owner_actor_id":"human:browser","description":"Initial human instructions",
      "acceptance_criteria":[{"id":"old","requirement":"Old acceptance","verification":"self_attested"}]}).encode()
    with urlopen(Request(base+"/api/tasks",data=body,
         headers={"Content-Type":"application/json"},method="POST"),timeout=10) as resp:
        return json.load(resp)["id"]
with sync_playwright() as p:
    browser=p.chromium.launch(headless=True)
    for width,height in [(1280,800),(390,844)]:
        tid=create_fixture()
        page=browser.new_page(viewport={"width":width,"height":height},device_scale_factor=1)
        page.goto(base+"/",wait_until="domcontentloaded")
        panel=page.locator(".task-revision-panel")
        panel.get_by_role("button",name="修订任务").wait_for(timeout=10000)
        panel.get_by_role("button",name="修订任务").click()
        editor=panel.locator("form.revision-editor")
        editor.get_by_label("修订原因").fill("当前检查需要对应新的证据")
        editor.get_by_label("标题").fill("Updated browser contract")
        editor.get_by_role("button",name="预览差异").click()
        page.get_by_text("变更预览（提交前）").wait_for(timeout=8000)
        assert editor.get_by_role("button",name="提交修订").is_enabled()
        editor.get_by_role("button",name="提交修订").click()
        page.wait_for_function("() => document.querySelector('.task-revision-header small')?.textContent?.includes('v2')",timeout=10000)
        panel.get_by_text("v2 · applied",exact=False).first.wait_for(timeout=10000)
        page.screenshot(path=f"task-revision-ui-validated-{width}.png",full_page=True)
        errors=page.locator("[role=alert]").all_text_contents()
        assert not errors,errors
        with urlopen(base+f"/api/tasks/{tid}/revisions") as resp:
            data=json.load(resp)
        assert data["active_version"]==2 and data["revisions"][0]["after"]["title"]=="Updated browser contract"
        print(json.dumps({"viewport":[width,height],"task":tid,"ui_version":data["active_version"],
              "history_count":len(data["revisions"]),"screenshot":f"task-revision-ui-validated-{width}.png"}))
        page.close()
    browser.close()
