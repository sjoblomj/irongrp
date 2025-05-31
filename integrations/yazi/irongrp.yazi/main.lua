local M = {}

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

    local status, _ = Command("irongrp"):args({
        "--input-path",
        tostring(job.file.url),
        "--output-path",
        tostring(cache_img_url),
        "--mode",
        "grp-to-png",
        "--tiled",
        "--use-transparency",
        "--max-width",
        (rt and rt.preview or PREVIEW).max_width,
    }):status()
    return true
end

return M
