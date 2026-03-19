local M = {}

local visible_set = nil
local file_signs = nil
local filter_cwd = nil
local event_subscribed = false
local placing_signs = false

local ns_id = vim.api.nvim_create_namespace("ArbiterNvimTreeSigns")

---@return table|nil
local function get_api()
  local ok, api = pcall(require, "nvim-tree.api")
  if not ok then
    return nil
  end
  return api
end

---@param cwd string
---@return string
local function normalize_cwd(cwd)
  return cwd:gsub("/$", "") .. "/"
end

---@param abs_path string?
---@param cwd string?
---@return string?
local function to_rel(abs_path, cwd)
  cwd = cwd or filter_cwd
  if not abs_path or not cwd then
    return nil
  end
  if abs_path:sub(1, #cwd) ~= cwd then
    return nil
  end
  local rel = abs_path:sub(#cwd + 1)
  if rel ~= "" then
    return rel
  end
  return nil
end

---@param data {bufnr: integer?, winnr: integer?}?
local function place_signs(data)
  if placing_signs or not file_signs or not filter_cwd then
    return
  end

  local bufnr = data and data.bufnr
  local winnr = data and data.winnr
  if not bufnr or not vim.api.nvim_buf_is_valid(bufnr) then
    return
  end
  if not winnr or not vim.api.nvim_win_is_valid(winnr) then
    return
  end

  placing_signs = true
  local saved_ei = vim.o.eventignore

  local ok, err = xpcall(function()
    vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)

    local api = get_api()
    if not api then
      return
    end

    local line_count = vim.api.nvim_buf_line_count(bufnr)
    vim.o.eventignore = "all"

    vim.api.nvim_win_call(winnr, function()
      local saved_cursor = vim.api.nvim_win_get_cursor(winnr)

      for lnum = 1, line_count do
        vim.api.nvim_win_set_cursor(winnr, { lnum, 0 })
        local node_ok, node = pcall(api.tree.get_node_under_cursor)
        if node_ok and node and node.type == "file" and node.absolute_path then
          local rel = to_rel(node.absolute_path)
          if rel and file_signs[rel] then
            local cfg = file_signs[rel]
            vim.api.nvim_buf_set_extmark(bufnr, ns_id, lnum - 1, 0, {
              sign_text = cfg.text,
              sign_hl_group = cfg.hl,
              priority = 50,
            })
          end
        end
      end

      vim.api.nvim_win_set_cursor(winnr, saved_cursor)
    end)
  end, debug.traceback)

  vim.o.eventignore = saved_ei
  placing_signs = false

  if not ok then
    vim.notify("[arbiter] place_signs failed: " .. tostring(err), vim.log.levels.WARN)
  end
end

--- Custom filter for nvim-tree.
---
--- Wire into your nvim-tree setup:
---
---   require("nvim-tree").setup({
---     filters = {
---       custom = require("arbiter.nvim_tree_adapter").filter,
---     },
---   })
---
--- No-op when arbiter is not in review mode.
---@param abs_path string
---@return boolean true to hide, false to show
function M.filter(abs_path)
  if not visible_set or not filter_cwd then
    return false
  end
  local rel = to_rel(abs_path)
  if not rel then
    return true
  end
  return not visible_set[rel]
end

--- Receives pre-computed state from Rust and refreshes the tree.
---
--- `visible_json` is a JSON array of relative paths (files and ancestor
--- directories) that should be visible. `signs_json` is a JSON object
--- mapping relative file paths to `{text, hl}` sign configuration.
---@param cwd string
---@param visible_json string
---@param signs_json string
function M.set_state(cwd, visible_json, signs_json)
  filter_cwd = normalize_cwd(cwd)
  visible_set = {}
  file_signs = {}

  local ok1, entries = pcall(vim.json.decode, visible_json)
  if ok1 and type(entries) == "table" then
    for _, p in ipairs(entries) do
      visible_set[p] = true
    end
  end

  local ok2, signs = pcall(vim.json.decode, signs_json)
  if ok2 and type(signs) == "table" then
    for path, cfg in pairs(signs) do
      file_signs[path] = cfg
    end
  end

  local api = get_api()

  if not event_subscribed and api then
    if api.events and api.events.Event and api.events.Event.TreeRendered then
      local sub_ok = pcall(api.events.subscribe, api.events.Event.TreeRendered, function(data)
        vim.schedule(function()
          place_signs(data)
        end)
      end)
      if sub_ok then
        event_subscribed = true
      end
    end
  end

  if api then
    local win = api.tree.winid()
    if win and vim.api.nvim_win_is_valid(win) then
      api.tree.reload()
      api.tree.expand_all()
    end
  end
end

--- Clears filter state and signs, closing the tree properly.
---
--- Calls `api.tree.close()` so nvim-tree cleans up its own internal
--- state before the arbiter tab is destroyed. Without this, wiping
--- the tab leaves nvim-tree in a broken state and hangs on re-open.
function M.clear()
  local api = get_api()
  if api then
    local win = api.tree.winid()
    if win and vim.api.nvim_win_is_valid(win) then
      local bufnr = vim.api.nvim_win_get_buf(win)
      vim.api.nvim_buf_clear_namespace(bufnr, ns_id, 0, -1)
    end
    api.tree.close()
  end

  visible_set = nil
  file_signs = nil
  filter_cwd = nil
end

--- Returns the relative path of the file node under the cursor.
---@param cwd string
---@return string?
function M.file_at_cursor(cwd)
  local api = get_api()
  if not api then
    return nil
  end
  local node = api.tree.get_node_under_cursor()
  if not node or node.type ~= "file" then
    return nil
  end
  return to_rel(node.absolute_path, normalize_cwd(cwd))
end

--- Returns the relative path of the directory node under the cursor.
---@param cwd string
---@return string?
function M.dir_at_cursor(cwd)
  local api = get_api()
  if not api then
    return nil
  end
  local node = api.tree.get_node_under_cursor()
  if not node or node.type ~= "directory" then
    return nil
  end
  return to_rel(node.absolute_path, normalize_cwd(cwd))
end

--- Toggles the directory node under the cursor.
function M.toggle_dir()
  local api = get_api()
  if not api then
    return
  end
  api.node.open.edit()
end

--- Scrolls the tree to reveal the given absolute path.
---@param abs_path string
function M.find_file(abs_path)
  local api = get_api()
  if not api then
    return
  end
  api.tree.find_file({ buf = abs_path, open = false, focus = false })
end

--- Opens nvim-tree rooted at the given directory in the current window.
---@param cwd string
---@return boolean
function M.open(cwd)
  local api = get_api()
  if not api then
    return false
  end
  local ok, err = pcall(api.tree.open, { path = cwd, current_window = true })
  if not ok then
    vim.notify("[arbiter] nvim-tree.open failed: " .. tostring(err), vim.log.levels.ERROR)
    return false
  end
  return true
end

return M
