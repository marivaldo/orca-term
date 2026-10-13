-- orca-term's Neovim client: the `:OrcaTerm <subcommand>` command. Neovim runs this file at startup.
--
-- It defines the command and nothing else: no global keymap, no autocommand, and no work until the
-- command runs. Each subcommand calls a module under lua/orca-term/.

---Each subcommand's name and what it does.
---@type table<string, fun()>
local subcommands = {
  sidebar = function()
    require('orca-term.sidebar').toggle()
  end,
}

---The subcommand names, sorted.
---@return string[]
local function names()
  local list = vim.tbl_keys(subcommands)
  table.sort(list)
  return list
end

vim.api.nvim_create_user_command('OrcaTerm', function(command)
  local name = command.fargs[1]
  local subcommand = subcommands[name]
  if not subcommand then
    local message = ('orca-term: unknown subcommand `%s`; use one of: %s'):format(
      name,
      table.concat(names(), ', ')
    )
    vim.notify(message, vim.log.levels.ERROR)
    return
  end
  subcommand()
end, {
  nargs = 1,
  desc = 'orca-term: run a subcommand, such as `:OrcaTerm sidebar`',
  complete = function(arg_lead)
    return vim.tbl_filter(function(name)
      return vim.startswith(name, arg_lead)
    end, names())
  end,
})
