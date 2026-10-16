-- orca-term's status strip as a lualine component (https://github.com/nvim-lualine/lualine.nvim):
--
--   require('lualine').setup({ sections = { lualine_x = { 'orca-term' } } })
--
-- lualine finds this file by the component's name. It draws the items `orca-term.strip` keeps
-- (the first draw starts the strip there: the live logic is not repeated here), each in the colour
-- of its OrcaTerm group over the background of the section it sits in, and nothing when the strip
-- is empty, so lualine hides the component and its separators. It adds itself to no section.
--
-- The colours: lualine draws a section in a highlight of its own, so an item drawn in its
-- OrcaTerm group straight away would lose the section's background. For each group, `init`
-- creates a lualine highlight with the group's foreground only, which lualine completes with the
-- section's background. lualine runs `init` again after every `:colorscheme`, so the colours
-- follow the theme. Modelled on lualine's own `diagnostics` component.

local highlights = require('orca-term.highlights')
local strip = require('orca-term.strip')

local M = require('lualine.component'):extend()

---Every group a strip item can be drawn in: each state's, the muted one of a state this client
---does not know yet, and the unseen one. Sorted, so lualine names its highlights the same way on
---every start.
---@return string[]
local function strip_groups()
  local groups = { 'OrcaTermMuted', 'OrcaTermUnseen' }
  for _, group in pairs(highlights.STATE_GROUPS) do
    table.insert(groups, group)
  end
  table.sort(groups)
  return groups
end

---The foreground of `group`, its links followed, as `#rrggbb`; `nil` when it has none.
---@param group string
---@return string?
local function foreground(group)
  local ok, attributes = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
  if not ok or type(attributes.fg) ~= 'number' then
    return nil
  end
  return ('#%06x'):format(attributes.fg)
end

---Sets the component up: lualine calls it on `setup()` and again after every `:colorscheme`.
---@param options table lualine's options for this component
function M:init(options)
  M.super.init(self, options)
  -- The groups are defined here too: after a `:colorscheme`, lualine may run this before the
  -- strip's own autocommand defines them again.
  highlights.define()
  ---The lualine highlight of each group that has a foreground; a group without one is drawn in
  ---the section's own colours.
  ---@type table<string, table>
  self.orca_term_highlights = {}
  for _, group in ipairs(strip_groups()) do
    local fg = foreground(group)
    if fg then
      self.orca_term_highlights[group] = self:create_hl({ fg = fg }, group)
    end
  end
end

---What lualine draws: each item in its colour, back to the section's colour after it, with a
---space between items; the empty string when the strip is empty.
---@return string
function M:update_status()
  local section = self:get_default_hl()
  local parts = {}
  for _, item in ipairs(strip.items()) do
    local highlight = self.orca_term_highlights[item.group]
    local colour = highlight and self:format_hl(highlight) or section
    table.insert(parts, colour .. item.text .. section)
  end
  return table.concat(parts, ' ')
end

return M
