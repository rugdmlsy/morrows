import pytest

import morrow_runtime.tools as tools_module
from morrow_runtime.settings import get_settings
from morrow_runtime.tools import build_mcp

CORE_TOOL_NAMES = {
    "workspace_open",
    "environment_get",
    "skill_list",
    "skill_load",
    "skill_read",
    "run_shell",
    "run_python",
    "shell_start",
    "shell_send",
    "shell_read",
    "shell_stop",
    "shell_list",
    "job_start",
    "job_list",
    "job_tail",
    "job_stop",
    "job_retry",
    "file_list",
    "file_tree",
    "file_glob",
    "file_grep",
    "file_read",
    "image_view",
    "link_create",
    "link_list",
    "link_revoke",
    "file_write",
    "file_edit",
    "file_delete",
    "file_patch",
    "secret_scan",
    "session_manage",
    "plan_manage",
    "mcp_manage",
    "mcp_tool_search",
    "mcp_tool_inspect",
    "mcp_tool_call",
    "browser_session",
    "browser_snapshot",
    "browser_act",
    "browser_run_script",
    "restart",
    "restart_status",
    "audit_tail",
}

APP_ONLY_TOOL_NAMES = {
    "live_workspace_reconnect",
}

REMOTE_DEPENDENT_TOOL_NAMES = {
    "mobile_action",
    "remote_manage",
    "remote_transfer",
}

REMOVED_TOOL_NAMES = {
    "search",
    "fetch",
    "open_live_workspace",
    "environment_info",
    "skills_list",
    "skill_read_file",
    "run_shell_tool",
    "run_python_tool",
    "shell_kill",
    "list_files",
    "tree_view",
    "glob_search",
    "grep_search",
    "read_file",
    "view_image",
    "create_file_link",
    "list_file_links",
    "revoke_file_link",
    "write_file",
    "edit_file",
    "delete_file_or_dir",
    "apply_patch",
    "version_info",
    "read_many_files",
    "multi_edit_file",
    "git_status_tool",
    "git_commit_tool",
    "remote_run_shell_tool",
    "remote_read_file",
    "remote_git_status_tool",
    "remote_copy_file",
    "remote_pull_file",
    "remote_push_file",
    "browser_screenshot_tool",
    "browser_pdf_tool",
    "browser_eval_tool",
    "browser_capture_tool",
    "browser_get_text_tool",
    "playwright_run_script_tool",
    "playwright_install_tool",
    "transfer_path",
    "remote_invite",
    "remote_list_machines",
    "remote_revoke_machine",
    "remote_rename_machine",
    "todo_read_tool",
    "todo_write_tool",
}


@pytest.mark.asyncio
async def test_mcp_tool_surface_is_stable(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "true")
    get_settings.cache_clear()

    tools = {tool.name: tool for tool in await build_mcp().list_tools()}

    assert set(tools) == CORE_TOOL_NAMES | REMOTE_DEPENDENT_TOOL_NAMES | APP_ONLY_TOOL_NAMES
    assert set(tools).isdisjoint(REMOVED_TOOL_NAMES)
    assert all(tool.outputSchema is not None for tool in tools.values())
    model_visible = {
        name
        for name, tool in tools.items()
        if "model" in (tool.meta.get("ui", {}).get("visibility") or ["model", "app"])
    }
    assert model_visible == CORE_TOOL_NAMES | REMOTE_DEPENDENT_TOOL_NAMES
    assert tools["live_workspace_reconnect"].meta["ui"]["visibility"] == ["app"]
    mobile_actions = set(tools["mobile_action"].inputSchema["properties"]["action"]["enum"])
    assert {
        "camera_capture",
        "photos_list",
        "photos_export",
        "network_status",
        "network_history",
        "dns_probe",
        "tcp_probe",
        "tls_probe",
        "http_probe",
        "bookmarks_list",
        "bookmark_import",
        "bookmark_export",
        "clipboard_status",
        "clipboard_write",
        "clipboard_read",
        "shared_inbox_import",
        "approval_prompt",
        "device_status",
        "sensor_snapshot",
        "last_scanned_code",
        "send_to_mobile",
        "inbox_list",
    } <= mobile_actions
    mobile_props = tools["mobile_action"].inputSchema["properties"]
    assert mobile_props["defer_if_offline"]["default"] is False
    assert mobile_props["defer_ttl_s"]["default"] == 86400


@pytest.mark.asyncio
async def test_mobile_action_requires_mobile_worker_capability(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "true")
    get_settings.cache_clear()

    class DesktopOnlyManager:
        def list_machines(self):
            return {"machines": [{"name": "desktop", "capabilities": ["shell", "files"]}]}

        async def call(self, *args, **kwargs):  # noqa: ANN002, ANN003
            raise AssertionError("mobile_action must not dispatch to a non-mobile worker")

    monkeypatch.setattr(tools_module, "remote_manager", lambda: DesktopOnlyManager())

    result = await build_mcp().call_tool(
        "mobile_action",
        {"machine": "desktop", "action": "battery", "timeout_s": 30},
    )

    assert result.isError is True
    assert result.structuredContent["ok"] is False
    assert "not a mobile worker" in result.structuredContent["message"]


@pytest.mark.asyncio
async def test_live_workspace_can_be_disabled_independently(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_LIVE_WORKSPACE_ENABLED", "false")
    monkeypatch.setenv("LOCAL_SHELL_MCP_UI_ENABLED", "true")
    get_settings.cache_clear()

    mcp = build_mcp()
    tools = {tool.name for tool in await mcp.list_tools()}
    resources = {str(resource.uri) for resource in await mcp.list_resources()}

    assert "workspace_open" not in tools
    assert "live_workspace_reconnect" not in tools
    assert not any("live-workspace" in uri for uri in resources)
    assert get_settings().ui_enabled is True


@pytest.mark.asyncio
async def test_logical_sessions_can_be_disabled_from_surface(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_LOGICAL_SESSIONS_ENABLED", "false")
    get_settings.cache_clear()

    mcp = build_mcp()
    tools = {tool.name: tool for tool in await mcp.list_tools()}

    assert "session_manage" not in tools
    assert "plan_manage" not in tools
    assert "logical_session_id" not in tools["run_shell"].inputSchema["properties"]
    assert "logical_session_id" not in tools["file_read"].inputSchema["properties"]
    assert "workspace_open" in tools
    assert "Logical Session" not in (mcp._mcp_server.instructions or "")  # noqa: SLF001


@pytest.mark.asyncio
async def test_remote_admin_tools_can_be_disabled_from_surface(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    get_settings.cache_clear()

    tools = {tool.name for tool in await build_mcp().list_tools()}

    assert tools == CORE_TOOL_NAMES | APP_ONLY_TOOL_NAMES
    assert tools.isdisjoint(REMOTE_DEPENDENT_TOOL_NAMES)


@pytest.mark.asyncio
async def test_machine_capable_tools_use_optional_machine_arguments(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "true")
    get_settings.cache_clear()

    tools = {tool.name: tool for tool in await build_mcp().list_tools()}
    machine_capable = {
        "workspace_open",
        "environment_get",
        "run_shell",
        "run_python",
        "shell_start",
        "shell_send",
        "shell_read",
        "shell_stop",
        "shell_list",
        "job_start",
        "job_list",
        "job_tail",
        "job_stop",
        "job_retry",
        "file_list",
        "file_tree",
        "file_glob",
        "file_grep",
        "file_read",
        "image_view",
        "file_write",
        "file_edit",
        "file_delete",
        "file_patch",
        "browser_session",
        "browser_snapshot",
        "browser_act",
        "browser_run_script",
        "restart",
        "restart_status",
    }

    for name in machine_capable:
        assert "machine" in tools[name].inputSchema["properties"], name
    transfer_properties = tools["remote_transfer"].inputSchema["properties"]
    assert {"source_machine", "destination_machine"} <= set(transfer_properties)

    edit_schema = tools["file_edit"].inputSchema
    edit_definition = edit_schema["$defs"]["TextEdit"]
    assert edit_schema["properties"]["edits"]["items"] == {"$ref": "#/$defs/TextEdit"}
    assert edit_definition["additionalProperties"] is False
    assert set(edit_definition["required"]) == {"old", "new"}
    assert edit_definition["properties"]["old"]["minLength"] == 1
    assert edit_definition["properties"]["replace_all"]["type"] == "boolean"


@pytest.mark.asyncio
async def test_job_notification_flag_is_opt_in_and_retry_can_override(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    get_settings.cache_clear()

    tools = {tool.name: tool for tool in await build_mcp().list_tools()}
    start_props = tools["job_start"].inputSchema["properties"]
    retry_props = tools["job_retry"].inputSchema["properties"]
    start_prop = start_props["notify_on_finish"]
    retry_prop = retry_props["notify_on_finish"]

    assert start_prop["default"] is False
    assert start_prop["type"] == "boolean"
    assert retry_prop["default"] is None
    assert {row.get("type") for row in retry_prop["anyOf"]} == {"boolean", "null"}
    for key in ("notify_title", "notify_summary_path"):
        assert start_props[key]["default"] is None
        assert retry_props[key]["default"] is None
        assert {row.get("type") for row in start_props[key]["anyOf"]} == {"string", "null"}
        assert {row.get("type") for row in retry_props[key]["anyOf"]} == {"string", "null"}


@pytest.mark.asyncio
async def test_key_tool_descriptions_guide_tool_choice(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "false")
    get_settings.cache_clear()

    tools = {tool.name: tool for tool in await build_mcp().list_tools()}

    assert "For long-running" in tools["run_shell"].description
    assert "purpose/explanation" in tools["run_shell"].description
    assert "Git" in tools["run_shell"].description
    assert "old must match exactly" in tools["file_edit"].description
    assert "recursive=true is required" in tools["file_delete"].description
    assert "high-entropy token" in tools["link_create"].description
    assert "native MCP image content" in tools["image_view"].description
    assert "existing file-transfer protocol" in tools["image_view"].description
    assert "tool surface stays fixed" in tools["skill_list"].description
    assert "exact name returned from skill_list" in tools["skill_load"].description
    assert "tools/list" in tools["mcp_tool_search"].description
    assert "mcp_tool_inspect" in tools["mcp_tool_search"].description
    assert "stable short refs" in tools["browser_snapshot"].description
    assert "browser_run_script" in tools["browser_act"].description


@pytest.mark.asyncio
async def test_risky_tools_accept_purpose_and_explanation(tmp_path, monkeypatch):
    monkeypatch.setenv("LOCAL_SHELL_MCP_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("LOCAL_SHELL_MCP_REMOTE_ENABLED", "true")
    get_settings.cache_clear()

    tools = {tool.name: tool for tool in await build_mcp().list_tools()}
    names = {
        "run_shell",
        "run_python",
        "shell_start",
        "job_start",
        "job_retry",
        "file_write",
        "file_edit",
        "file_delete",
        "file_patch",
        "restart",
        }

    for name in names:
        properties = tools[name].inputSchema["properties"]
        assert "purpose" in properties, name
        assert "explanation" in properties, name
