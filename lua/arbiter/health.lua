local M = {}

function M.check()
  vim.health.start("arbiter")

  local build = require("arbiter.build")
  local system = require("arbiter.system")

  vim.health.info("Platform: " .. system.get_triple())
  vim.health.info("Library extension: " .. system.get_lib_extension())

  local binary_path = build.get_binary_path()
  vim.health.info("Expected binary: " .. binary_path)

  local stat = vim.uv.fs_stat(binary_path)
  if stat and stat.type == "file" then
    vim.health.ok("Binary found at " .. binary_path)
  else
    vim.health.error(
      "Binary not found at " .. binary_path,
      {
        "Run `cargo build --release` in the plugin directory",
        'Or run `:lua require("arbiter.build").download_or_build_binary()`',
      }
    )
  end

  local loader, load_err = package.loadlib(binary_path, "luaopen_arbiter")
  if loader then
    vim.health.ok("Binary loads successfully")
  elseif stat then
    vim.health.error("Binary exists but failed to load: " .. (load_err or "unknown error"))
  end

  if vim.fn.executable("cargo") == 1 then
    vim.health.ok("cargo found on PATH")
  else
    vim.health.warn("cargo not found on PATH (needed to build from source)")
  end

  if vim.fn.executable("git") == 1 then
    vim.health.ok("git found on PATH")
  else
    vim.health.error("git not found on PATH (required for diffs)")
  end

  local has_agent = vim.fn.executable("agent") == 1
  local has_claude = vim.fn.executable("claude") == 1
  if has_agent then
    vim.health.ok("Cursor CLI (`agent`) found on PATH")
  end
  if has_claude then
    vim.health.ok("Claude Code CLI found on PATH")
  end
  if not has_agent and not has_claude then
    vim.health.error(
      "No backend CLI found",
      {
        "Install the Cursor CLI (`agent`): https://docs.cursor.com/cli",
        "Or install Claude Code: `npm install -g @anthropic-ai/claude-code`",
      }
    )
  end

  local search_paths = build.get_search_paths()
  vim.health.info("Library search paths:")
  for _, p in ipairs(search_paths) do
    local resolved = vim.fn.fnamemodify(p:gsub("%?", "arbiter"), ":p")
    local found = vim.uv.fs_stat(resolved) and "found" or "not found"
    vim.health.info("  " .. resolved .. " (" .. found .. ")")
  end
end

return M
