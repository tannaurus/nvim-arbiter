local build = require("arbiter.build")

local is_windows = (vim.uv or vim.loop).os_uname().sysname:lower():match("windows")

local function resolve_path(path)
  local resolved = vim.fn.fnamemodify(path, ":p")
  if is_windows then
    resolved = resolved:gsub("/", "\\")
  end
  return resolved
end

local function try_load_library()
  local paths = build.get_search_paths()

  for _, path_pattern in ipairs(paths) do
    local actual_path = resolve_path(path_pattern:gsub("%?", "arbiter"))
    local stat = vim.uv.fs_stat(actual_path)
    if stat and stat.type == "file" then
      local loader, err = package.loadlib(actual_path, "luaopen_arbiter")
      if err then
        return nil, string.format("Error loading library from %s: %s", actual_path, err)
      end
      if loader then
        return loader()
      end
    end
  end

  return nil, "No valid library found in any search path"
end

local backend, load_err = try_load_library()

if not backend then
  build.download_or_build_binary()
  backend, load_err = try_load_library()
end

if not backend then
  local paths = build.get_search_paths()
  local resolved = {}
  for _, p in ipairs(paths) do
    table.insert(resolved, resolve_path(p:gsub("%?", "arbiter")))
  end

  error(string.format(
    "[arbiter] Failed to load native module.\nError: %s\nSearched paths:\n%s\n"
      .. "Build with `cargo build --release` or run:\n"
      .. '  :lua require("arbiter.build").download_or_build_binary()',
    tostring(load_err),
    table.concat(resolved, "\n")
  ))
end

return backend
