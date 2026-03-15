local M = {}
local system = require("arbiter.system")

local GITHUB_REPO = "tannaurus/nvim-arbiter"

local function get_plugin_dir()
  return vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":h:h:h")
end

local function get_binary_dir(plugin_dir)
  return plugin_dir .. "/target/release"
end

local function get_binary_name()
  local ext = system.get_lib_extension()
  if ext == "dll" then
    return "arbiter." .. ext
  end
  return "libarbiter." .. ext
end

local function get_binary_path(plugin_dir)
  return get_binary_dir(plugin_dir) .. "/" .. get_binary_name()
end

local function binary_exists(plugin_dir)
  local path = get_binary_path(plugin_dir)
  local stat = vim.uv.fs_stat(path)
  if stat and stat.type == "file" then
    return true
  end

  local tmp_path = path .. ".tmp"
  local tmp_stat = vim.uv.fs_stat(tmp_path)
  if tmp_stat and tmp_stat.type == "file" then
    local loader = package.loadlib(tmp_path, "luaopen_arbiter")
    if not loader then
      vim.uv.fs_unlink(tmp_path)
      return false
    end
    local ok = vim.uv.fs_rename(tmp_path, path)
    return ok ~= nil
  end

  return false
end

local function get_current_version(plugin_dir, callback)
  vim.system({ "git", "describe", "--tags", "--exact-match", "HEAD" }, { cwd = plugin_dir }, function(result)
    if result.code ~= 0 or not result.stdout or result.stdout == "" then
      callback(nil)
      return
    end
    callback(result.stdout:gsub("%s+", ""))
  end)
end

local function mkdir_recursive(path, callback)
  vim.uv.fs_stat(path, function(err, stat)
    if not err and stat then
      callback(true, nil)
      return
    end

    local parent = vim.fn.fnamemodify(path, ":h")
    if parent == path or parent == "" or parent == "." then
      callback(false, "Cannot create root directory")
      return
    end

    mkdir_recursive(parent, function(parent_ok, parent_err)
      if not parent_ok then
        callback(false, parent_err)
        return
      end

      vim.uv.fs_mkdir(path, 493, function(mkdir_err)
        if mkdir_err and not mkdir_err:match("EEXIST") then
          callback(false, "Failed to create directory: " .. mkdir_err)
          return
        end
        callback(true, nil)
      end)
    end)
  end)
end

local function download_file(url, output_path, callback)
  local dir = vim.fn.fnamemodify(output_path, ":h")
  mkdir_recursive(dir, function(mkdir_ok, mkdir_err)
    if not mkdir_ok then
      callback(false, mkdir_err)
      return
    end

    vim.system({
      "curl",
      "--fail",
      "--location",
      "--silent",
      "--show-error",
      "--output",
      output_path,
      url,
    }, {}, function(result)
      if result.code ~= 0 then
        callback(false, "Download failed: " .. (result.stderr or "unknown error"))
        return
      end
      callback(true, nil)
    end)
  end)
end

local function download_from_github(version, binary_path, callback)
  local triple = system.get_triple()
  local ext = system.get_lib_extension()
  local binary_name = triple .. "." .. ext
  local url = string.format(
    "https://github.com/%s/releases/download/%s/%s",
    GITHUB_REPO, version, binary_name
  )

  vim.schedule(function()
    vim.notify(
      "[arbiter] Downloading prebuilt binary for " .. version .. "...",
      vim.log.levels.INFO
    )
  end)

  local tmp_path = binary_path .. ".tmp"

  download_file(url, tmp_path, function(success, err)
    if not success then
      vim.uv.fs_unlink(tmp_path)
      callback(false, err)
      return
    end

    vim.schedule(function()
      local loader, load_err = package.loadlib(tmp_path, "luaopen_arbiter")
      if not loader then
        vim.uv.fs_unlink(tmp_path)
        callback(false, "Downloaded binary is not valid: " .. (load_err or "unknown error"))
        return
      end

      local rename_ok, rename_err = vim.uv.fs_rename(tmp_path, binary_path)
      if not rename_ok then
        if vim.uv.os_uname().sysname:lower():match("windows") then
          vim.notify(
            "[arbiter] Binary downloaded to " .. tmp_path
              .. ". Restart Neovim to apply the update.",
            vim.log.levels.WARN
          )
          callback(true, nil)
        else
          vim.uv.fs_unlink(tmp_path)
          callback(false, "Failed to install binary: " .. (rename_err or "unknown error"))
        end
        return
      end

      vim.notify("[arbiter] Binary downloaded successfully.", vim.log.levels.INFO)
      callback(true, nil)
    end)
  end)
end

function M.build_binary(callback)
  local plugin_dir = get_plugin_dir()

  if vim.fn.executable("cargo") ~= 1 then
    callback(false, "cargo not found. Install the Rust toolchain from https://rustup.rs/")
    return
  end

  vim.schedule(function()
    vim.notify("[arbiter] Building from source...", vim.log.levels.INFO)
  end)

  vim.system({ "cargo", "build", "--release" }, { cwd = plugin_dir }, function(result)
    if result.code ~= 0 then
      callback(false, "cargo build failed: " .. (result.stderr or "unknown error"))
      return
    end

    vim.schedule(function()
      vim.notify("[arbiter] Build complete.", vim.log.levels.INFO)
    end)
    callback(true, nil)
  end)
end

function M.ensure_built(opts, callback)
  opts = opts or {}
  local plugin_dir = get_plugin_dir()

  if binary_exists(plugin_dir) and not opts.force then
    callback(true, nil)
    return
  end

  local binary_path = get_binary_path(plugin_dir)

  local function on_version(target_version)
    if not target_version then
      M.build_binary(callback)
      return
    end

    download_from_github(target_version, binary_path, function(download_ok, download_err)
      if download_ok then
        callback(true, nil)
        return
      end

      vim.schedule(function()
        vim.notify(
          "[arbiter] Download failed: " .. (download_err or "unknown") .. ". Falling back to cargo build.",
          vim.log.levels.WARN
        )
      end)

      M.build_binary(callback)
    end)
  end

  if opts.version then
    on_version(opts.version)
  else
    get_current_version(plugin_dir, on_version)
  end
end

function M.download_or_build_binary()
  local done = false
  local fatal_error = nil

  M.ensure_built({ force = true }, function(success, err)
    if not success then
      fatal_error = "[arbiter] " .. (err or "unknown error")
    end
    done = true
  end)

  local timeout_ms = 1000 * 60 * 5
  local ok, wait_err = vim.wait(timeout_ms, function() return done end, 100)
  if not ok and wait_err == -2 then
    error("[arbiter] download_or_build_binary timed out")
  end

  if fatal_error then error(fatal_error) end
end

function M.get_binary_path()
  return get_binary_path(get_plugin_dir())
end

function M.get_binary_cpath_component()
  local plugin_dir = get_plugin_dir()
  local binary_dir = get_binary_dir(plugin_dir)
  local ext = system.get_lib_extension()
  return binary_dir .. "/lib?." .. ext
end

function M.get_search_paths()
  local plugin_dir = get_plugin_dir()
  local ext = system.get_lib_extension()

  local paths = {
    M.get_binary_cpath_component(),
    plugin_dir .. "/target/release/libarbiter." .. ext,
    plugin_dir .. "/target/release/arbiter." .. ext,
  }

  local cargo_target_dir = os.getenv("CARGO_TARGET_DIR")
  if cargo_target_dir then
    table.insert(paths, cargo_target_dir .. "/release/libarbiter." .. ext)
    table.insert(paths, cargo_target_dir .. "/release/arbiter." .. ext)
  end

  return paths
end

return M
