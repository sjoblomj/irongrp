local M = {}

local MAX_HISTORY = 9

local function expand_path(p)
	if type(p) ~= "string" or p == "" then
		return p
	end
	if p == "~" then
		return os.getenv("HOME") or p
	end
	if p:sub(1, 2) == "~/" then
		local home = os.getenv("HOME")
		if home then
			p = home .. p:sub(2)
		end
	end
	p = p:gsub("%${([A-Za-z_][A-Za-z0-9_]*)}", function(name)
		return os.getenv(name) or ("${" .. name .. "}")
	end)
	p = p:gsub("%$([A-Za-z_][A-Za-z0-9_]*)", function(name)
		return os.getenv(name) or ("$" .. name)
	end)
	return p
end

local function history_path()
	local state_home = os.getenv("XDG_STATE_HOME")
	if not state_home or state_home == "" then
		local home = os.getenv("HOME") or ""
		state_home = home .. "/.local/state"
	end
	return state_home .. "/yazi/irongrp-palettes"
end

local function load_history()
	local list = {}
	local f = io.open(history_path(), "r")
	if not f then
		return list
	end
	for line in f:lines() do
		if line ~= "" then
			table.insert(list, line)
		end
	end
	f:close()
	return list
end

local function save_history(list)
	local f = io.open(history_path(), "w")
	if not f then
		return
	end
	for i = 1, math.min(#list, MAX_HISTORY) do
		f:write(list[i])
		f:write("\n")
	end
	f:close()
end

local function add_to_history(path)
	if type(path) ~= "string" or path == "" then
		return
	end
	local list = load_history()
	local out = { path }
	for _, p in ipairs(list) do
		if p ~= path and #out < MAX_HISTORY then
			table.insert(out, p)
		end
	end
	save_history(out)
end

local function file_exists(path)
	if type(path) ~= "string" or path == "" then
		return false
	end
	local f = io.open(path, "r")
	if not f then
		return false
	end
	f:close()
	return true
end

local function effective_palette(explicit)
	if explicit == false then
		return nil
	elseif type(explicit) == "string" and explicit ~= "" then
		return explicit
	end
	local history = load_history()
	if #history > 0 and file_exists(history[1]) then
		return history[1]
	end
	return nil
end

local set_palette = ya.sync(function(st, url, path)
	st.palettes = st.palettes or {}
	if path == nil or path == "" then
		st.palettes[url] = nil
	else
		st.palettes[url] = expand_path(path)
	end
end)

local set_greyscale = ya.sync(function(st, url)
	st.palettes = st.palettes or {}
	st.palettes[url] = false
end)

local get_palette = ya.sync(function(st, url)
	return (st.palettes or {})[url]
end)

local get_hovered_url = ya.sync(function()
	local h = cx.active.current.hovered
	return h and tostring(h.url) or nil
end)

local refresh_preview = ya.sync(function()
	local h = cx.active.current.hovered
	if h then
		ya.manager_emit("peek", { 0, only_if = h.url, force = true })
	end
end)

function M:peek(job)
	local cache_img_url = ya.file_cache({ file = job.file, skip = 0 })

	local ok, err = self:preload(job)
	if not ok or err then
		return
	end

    local rendered_img_rect = cache_img_url
			and ya.image_show(
		        cache_img_url:join("all_frames.png"),
				ui.Rect({
					x = job.area.x,
					y = job.area.y,
					w = job.area.w,
					h = job.area.h,
				})
			)
		or nil
end

function M:seek(job)
	local h = cx.active.current.hovered
	if h and h.url == job.file.url then
		local step = ya.clamp(-10, job.units, 10)
		ya.manager_emit("peek", {
			math.max(0, cx.active.preview.skip + job.units),
			only_if = job.file.url,
		})
	end
end

function M:preload(job)
	local cache_img_url = ya.file_cache({ file = job.file, skip = 0 })
	if not cache_img_url then
		return true
	end

    local args = {
        "grp-to-png",
        tostring(job.file.url),
        tostring(cache_img_url),
        "--tiled",
        "--transparent",
        "--force",
        "--max-width",
        (rt and rt.preview or PREVIEW).max_width,
    }

    local pal = effective_palette(get_palette(tostring(job.file.url)))
    if pal then
        table.insert(args, "--palette")
        table.insert(args, pal)
    end

    local status, _ = Command("irongrp"):args(args):status()
    return true
end

local function pick_palette_action()
	local history = load_history()
	local cands, actions = {}, {}

	for i, p in ipairs(history) do
		if i > MAX_HISTORY then
			break
		end
		table.insert(cands, { on = tostring(i), desc = p })
		table.insert(actions, { kind = "pick", path = p })
	end

	table.insert(cands, { on = "p", desc = "Enter palette path…" })
	table.insert(actions, { kind = "new" })

	table.insert(cands, { on = "g", desc = "Greyscale palette" })
	table.insert(actions, { kind = "greyscale" })

	local idx = ya.which({ cands = cands })
	return idx and actions[idx] or nil
end

function M:entry(job)
	local url = get_hovered_url()
	if not url then
		return
	end

	local choice = pick_palette_action()
	if not choice then
		return
	end

	if choice.kind == "greyscale" then
		set_greyscale(url)
		refresh_preview()
		return
	end

	local new_path
	if choice.kind == "pick" then
		new_path = choice.path
	elseif choice.kind == "new" then
		local current = get_palette(url)
		local value, event = ya.input({
			title = "Palette file path (empty for greyscale):",
			value = (type(current) == "string" and current) or "",
			position = { "top-center", y = 3, w = 60 },
		})
		if event ~= 1 then
			return
		end
		if value == "" then
			set_greyscale(url)
			refresh_preview()
			return
		end
		new_path = expand_path(value)
	end

	set_palette(url, new_path)
	if new_path and new_path ~= "" then
		add_to_history(new_path)
	end
	refresh_preview()
end

return M
