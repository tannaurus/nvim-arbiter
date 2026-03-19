-- Pure-logic tests for the nvim-tree adapter filter.
-- Run with: lua tests/lua/test_nvim_tree_filter.lua
--
-- Tests exercise the filter function without requiring nvim or nvim-tree.
-- The visible set and sign config are passed as pre-computed JSON from Rust,
-- so tests focus on the O(1) set-lookup filter behavior and state transitions.

local passed = 0
local failed = 0

local function assert_eq(actual, expected, msg)
  if actual ~= expected then
    failed = failed + 1
    io.stderr:write(string.format("FAIL: %s\n  expected: %s\n  actual:   %s\n", msg, tostring(expected), tostring(actual)))
  else
    passed = passed + 1
  end
end

-- Stub vim for standalone lua
if not vim then
  vim = {
    json = {
      decode = function(s)
        -- Minimal JSON parser for arrays and objects
        if s == "[]" then return {} end
        if s == "{}" then return {} end
        -- Try array: ["a","b","c"]
        if s:sub(1, 1) == "[" then
          local t = {}
          for item in s:gmatch('"([^"]*)"') do
            table.insert(t, item)
          end
          return t
        end
        -- Try object: {"key":{"text":"x","hl":"Y"}, ...} or {"key":true, ...}
        local t = {}
        -- Match "key":{"text":"...","hl":"..."}
        for key, text, hl in s:gmatch('"([^"]+)"%s*:%s*{%s*"text"%s*:%s*"([^"]*)"%s*,%s*"hl"%s*:%s*"([^"]*)"%s*}') do
          t[key] = { text = text, hl = hl }
        end
        return t
      end,
    },
    notify = function() end,
    log = { levels = { ERROR = 4 } },
    g = {},
    o = {},
    api = {
      nvim_create_namespace = function() return 1 end,
      nvim_buf_clear_namespace = function() end,
      nvim_buf_is_valid = function() return false end,
      nvim_win_is_valid = function() return false end,
      nvim_win_get_buf = function() return 0 end,
    },
  }
end

-- Stub nvim-tree API
package.preload["nvim-tree.api"] = function()
  return {
    tree = {
      reload = function() end,
      open = function() return true end,
      get_node_under_cursor = function() return nil end,
      find_file = function() end,
      winid = function() return nil end,
    },
    node = {
      open = { edit = function() end },
    },
    events = {
      Event = { TreeRendered = "TreeRendered" },
      subscribe = function() end,
    },
  }
end

-- Load the adapter
package.path = "lua/?.lua;" .. package.path
local adapter = dofile("lua/arbiter/nvim_tree_adapter.lua")

-- Helper: build visible JSON from a list of paths (simulates Rust output)
local function visible_json(paths)
  if #paths == 0 then return "[]" end
  local parts = {}
  for _, p in ipairs(paths) do
    table.insert(parts, '"' .. p .. '"')
  end
  return "[" .. table.concat(parts, ",") .. "]"
end

-- Test: filter inactive returns false for everything
adapter.clear()
assert_eq(adapter.filter("/home/user/project/src/main.rs"), false, "inactive: any file passes")
assert_eq(adapter.filter("/home/user/project/README.md"), false, "inactive: root file passes")

-- Test: filter active with visible set
adapter.set_state(
  "/home/user/project",
  visible_json({"src", "src/main.rs", "src/lib.rs", "README.md"}),
  "{}"
)
assert_eq(adapter.filter("/home/user/project/src/main.rs"), false, "visible file passes")
assert_eq(adapter.filter("/home/user/project/src/lib.rs"), false, "visible file passes (lib)")
assert_eq(adapter.filter("/home/user/project/README.md"), false, "visible root file passes")
assert_eq(adapter.filter("/home/user/project/src"), false, "visible ancestor dir passes")

-- Test: non-visible paths are hidden
assert_eq(adapter.filter("/home/user/project/src/utils.rs"), true, "non-visible file hidden")
assert_eq(adapter.filter("/home/user/project/Cargo.toml"), true, "non-visible root file hidden")
assert_eq(adapter.filter("/home/user/project/tests"), true, "non-visible dir hidden")

-- Test: deeply nested visible set
adapter.set_state(
  "/home/user/project",
  visible_json({"a", "a/b", "a/b/c", "a/b/c/d", "a/b/c/d/deep.rs"}),
  "{}"
)
assert_eq(adapter.filter("/home/user/project/a"), false, "ancestor a passes")
assert_eq(adapter.filter("/home/user/project/a/b"), false, "ancestor a/b passes")
assert_eq(adapter.filter("/home/user/project/a/b/c"), false, "ancestor a/b/c passes")
assert_eq(adapter.filter("/home/user/project/a/b/c/d"), false, "ancestor a/b/c/d passes")
assert_eq(adapter.filter("/home/user/project/a/b/c/d/deep.rs"), false, "deep file passes")
assert_eq(adapter.filter("/home/user/project/a/b/c/d/other.rs"), true, "non-visible sibling hidden")
assert_eq(adapter.filter("/home/user/project/a/x"), true, "non-visible subdir hidden")

-- Test: root-level file only
adapter.set_state(
  "/home/user/project",
  visible_json({"Makefile"}),
  "{}"
)
assert_eq(adapter.filter("/home/user/project/Makefile"), false, "root file passes")
assert_eq(adapter.filter("/home/user/project/Dockerfile"), true, "other root file hidden")
assert_eq(adapter.filter("/home/user/project/src"), true, "dir hidden when only root file visible")

-- Test: path outside cwd
adapter.set_state(
  "/home/user/project",
  visible_json({"src", "src/main.rs"}),
  "{}"
)
assert_eq(adapter.filter("/other/path/file.rs"), true, "path outside cwd hidden")

-- Test: empty visible set hides everything
adapter.set_state("/home/user/project", "[]", "{}")
assert_eq(adapter.filter("/home/user/project/src/main.rs"), true, "empty set hides all files")
assert_eq(adapter.filter("/home/user/project/src"), true, "empty set hides all dirs")

-- Test: clear restores inactive state
adapter.set_state(
  "/home/user/project",
  visible_json({"src", "src/main.rs"}),
  "{}"
)
assert_eq(adapter.filter("/home/user/project/src/utils.rs"), true, "before clear: non-visible hidden")
adapter.clear()
assert_eq(adapter.filter("/home/user/project/src/utils.rs"), false, "after clear: everything passes")
assert_eq(adapter.filter("/home/user/project/anything"), false, "after clear: anything passes")

-- Test: cwd with trailing slash handled
adapter.set_state(
  "/home/user/project/",
  visible_json({"src", "src/main.rs"}),
  "{}"
)
assert_eq(adapter.filter("/home/user/project/src/main.rs"), false, "trailing slash cwd: visible file passes")
assert_eq(adapter.filter("/home/user/project/src/other.rs"), true, "trailing slash cwd: non-visible hidden")

-- Test: multiple dirs with shared ancestors
adapter.set_state(
  "/proj",
  visible_json({"src", "src/a.rs", "tests", "tests/b.rs", "README.md"}),
  "{}"
)
assert_eq(adapter.filter("/proj/src/a.rs"), false, "multi: src/a passes")
assert_eq(adapter.filter("/proj/tests/b.rs"), false, "multi: tests/b passes")
assert_eq(adapter.filter("/proj/README.md"), false, "multi: README passes")
assert_eq(adapter.filter("/proj/src"), false, "multi: src dir passes")
assert_eq(adapter.filter("/proj/tests"), false, "multi: tests dir passes")
assert_eq(adapter.filter("/proj/src/z.rs"), true, "multi: non-visible in src hidden")
assert_eq(adapter.filter("/proj/benches"), true, "multi: non-visible dir hidden")

-- Summary
print(string.format("\n%d passed, %d failed", passed, failed))
if failed > 0 then
  os.exit(1)
end
