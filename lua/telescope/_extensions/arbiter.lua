local has_telescope, telescope = pcall(require, "telescope")
if not has_telescope then
  return
end

local pickers = require("telescope.pickers")
local finders = require("telescope.finders")
local conf = require("telescope.config").values
local actions = require("telescope.actions")
local action_state = require("telescope.actions.state")
local entry_display = require("telescope.pickers.entry_display")

local function get_review_info()
  local ok, arbiter = pcall(require, "arbiter")
  if not ok then
    vim.notify("[arbiter] plugin not loaded", vim.log.levels.ERROR)
    return nil
  end
  local info = arbiter.review_files()
  if not info then
    vim.notify("[arbiter] no active review", vim.log.levels.WARN)
    return nil
  end
  return info
end

local status_icon = {
  approved = "✓",
  unreviewed = "·",
}

local status_hl = {
  approved = "DiagnosticOk",
  unreviewed = "Comment",
}

local function review_files(opts)
  opts = opts or {}
  local info = get_review_info()
  if not info then
    return
  end

  local displayer = entry_display.create({
    separator = " ",
    items = {
      { width = 1 },
      { remaining = true },
    },
  })

  local function make_display(entry)
    return displayer({
      { status_icon[entry.status] or "·", status_hl[entry.status] or "Comment" },
      entry.value,
    })
  end

  pickers
    .new(opts, {
      prompt_title = "Review Files",
      finder = finders.new_table({
        results = (function()
          local results = {}
          for _, f in ipairs(info.files) do
            table.insert(results, { path = f.path, status = f.status })
          end
          return results
        end)(),
        entry_maker = function(item)
          return {
            value = item.path,
            display = make_display,
            ordinal = item.path,
            status = item.status,
            filename = info.cwd .. "/" .. item.path,
          }
        end,
      }),
      sorter = conf.generic_sorter(opts),
      previewer = conf.file_previewer(opts),
      attach_mappings = function(prompt_bufnr)
        actions.select_default:replace(function()
          local entry = action_state.get_selected_entry()
          actions.close(prompt_bufnr)
          vim.cmd("ArbiterFile " .. vim.fn.fnameescape(entry.value))
        end)
        return true
      end,
    })
    :find()
end

local function review_grep(opts)
  opts = opts or {}
  local info = get_review_info()
  if not info then
    return
  end

  local abs_files = {}
  for _, f in ipairs(info.files) do
    table.insert(abs_files, info.cwd .. "/" .. f.path)
  end

  require("telescope.builtin").live_grep(vim.tbl_extend("force", opts, {
    search_dirs = abs_files,
    prompt_title = "Review Grep",
    attach_mappings = function(prompt_bufnr)
      actions.select_default:replace(function()
        local entry = action_state.get_selected_entry()
        actions.close(prompt_bufnr)
        if entry and entry.filename then
          local rel = entry.filename
          local prefix = info.cwd .. "/"
          if rel:sub(1, #prefix) == prefix then
            rel = rel:sub(#prefix + 1)
          end
          local lnum = entry.lnum or 1
          vim.cmd("ArbiterFile " .. vim.fn.fnameescape(rel) .. " " .. lnum)
        end
      end)
      return true
    end,
  }))
end

return telescope.register_extension({
  exports = {
    review_files = review_files,
    review_grep = review_grep,
  },
})
