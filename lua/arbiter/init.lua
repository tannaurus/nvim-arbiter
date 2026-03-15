-- Loader for arbiter native module.
-- Loads the dylib directly from the cargo build output to avoid
-- code signature invalidation from file copies on macOS.
local source = debug.getinfo(1, "S").source:sub(2)
local dir = vim.fs.dirname(source)
local plugin_root = vim.fs.dirname(vim.fs.dirname(dir))

local function find_or_build()
  local release_dir = plugin_root .. "/target/release"

  local dylib_path = release_dir .. "/libarbiter.dylib"
  if vim.uv.fs_stat(dylib_path) then
    return dylib_path
  end

  local so_path = release_dir .. "/libarbiter.so"
  if vim.uv.fs_stat(so_path) then
    return so_path
  end

  vim.notify("[arbiter] Building from source...", vim.log.levels.INFO)
  local cmd = "cd " .. vim.fn.shellescape(plugin_root) .. " && cargo build --release"
  vim.fn.system(cmd)
  if vim.v.shell_error ~= 0 then
    vim.notify("[arbiter] Build failed. Run `cargo build --release` in " .. plugin_root, vim.log.levels.ERROR)
    return nil
  end

  if vim.uv.fs_stat(dylib_path) then
    return dylib_path
  end
  if vim.uv.fs_stat(so_path) then
    return so_path
  end

  vim.notify("[arbiter] Build succeeded but library not found", vim.log.levels.ERROR)
  return nil
end

local lib_path = find_or_build()
if not lib_path then
  error("[arbiter] Could not build or find native module")
end

local loader = package.loadlib(lib_path, "luaopen_arbiter")
if not loader then
  error("[arbiter] Failed to load " .. lib_path)
end

return loader()
