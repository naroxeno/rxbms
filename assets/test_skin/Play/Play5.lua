-- FAm Breeze Skin for Beatoraja
-- Created by FAm Renderer

main_state = require("main_state")

local function append_all(list, list1)
	for i, v in ipairs(list1) do
		table.insert(list, v)
	end
end

local property = {
	{name = "Lane Side - 轨道位置", item = {
		{name = "1P", op = 900},
		{name = "2P", op = 901}
	}},
	{name = "Ghost Display - 分数差显示", def = "Off", item = {
		{name = "Off", op = 910},
		{name = "Personal Best - 个人最高分", op = 911},
		{name = "Target - 目标分数", op = 912},
	}},
	{name = "Fast/Slow Display - Fast/Slow显示", def = "Off", item = {
		{name = "Off", op = 920},
		{name = "On", op = 921}
	}},
	{name = "Input Device - 输入设备", item = {
		{name = "Keyboard - 键盘", op = 940},
		{name = "IIDX Controller - IIDX手台", op = 941}
	}},
	{name = "Pure Mode - 纯净模式", item = {
		{name = "Off", op = 930},
		{name = "On", op = 931}
	}},

}

local function is1p()
	return skin_config.option["Lane Side - 轨道位置"] == 900
end

local function getLaneSide()
	if skin_config.option["Lane Side - 轨道位置"] == 900 then
		return 0
	else
		return 1
	end
end

local function ghostOption()
	return skin_config.option["Ghost Display - 分数差显示"] - 910
end

local function isFastSlowOn()
	return skin_config.option["Fast/Slow Display - Fast/Slow显示"] == 920
end

local function isPureModeOff()
	return skin_config.option["Pure Mode - 纯净模式"] == 930
end

local function isLongChart()
	return main_state.number(74) >= 5000
end

local function isLargeBPM()
	return main_state.number(90) >= 1000000
end

local filepath = {
	{name = "Background - 背景", path = "Background/*.png", def = "Default"},
	{name = "Notes - 音符样式", path = "Parts/Notes/*.png", def = "Default"},
	{name = "Key Beam - 轨道光效", path = "Parts/Key Beam/*.png", def = "Default"},
	{name = "Bomb - 打击效果", path = "Parts/Bomb/*.png", def = "Default"},
	{name = "Judge - 判定文字样式", path = "Parts/Judge/*.png", def = "Default"},
	{name = "Judge Line - 判定线样式", path = "Parts/Judge Line/*.png", def = "Default"},
	{name = "Groove Gauge - 生命条样式", path = "Parts/Gauge/*.png", def = "Default"},
	{name = "Lane Cover - 挡板样式", path = "Parts/Lane Cover/*.png", def = "Default"},
	{name = "Lift Cover - 判定线提升样式", path = "Parts/Lift Cover/*.png", def = "Default"},
}

local offset = {
	{name = "Lane Line Transparency - 轨道分隔线透明度 (0-100)", id = 50, a = 0},
}

local header = {
	type = 1,
	name = "FAm Breeze 1.1",
	w = 1920,
	h = 1080,
	loadend = 3000,
	playstart = 1500,
	scene = 3600000,
	input = 0,
	close = 3000,
	fadeout = 1000,
	property = property,
	filepath = filepath,
	offset = offset
}

local function main()
    local skin = {}
	for k, v in pairs(header) do
		skin[k] = v
	end

	local laneSide = getLaneSide()
	local laneBrightness = 255 - 2.55 * skin_config.offset["Lane Line Transparency - 轨道分隔线透明度 (0-100)"].a

	local geometry = {
		note_h = 30,
		note_w_w = 90,
		note_b_w = 80,
		note_s_w = 140,
		note_size = {},
		note_relative_x = {},
		lane_center_relative_x = {},
		lane_x = 85 + 982 * laneSide,
		lane_x_available = 85 + 1160 * laneSide,
		lane_5k_x = 679 + 388 * laneSide,
		lane_y = 200,
		lane_w = 768,
		lane_w_available = 590,
		lane_h = 880,
		judge_y = 400,
		fs_center_x = 418 + 982 * laneSide,
		ghost_center_x = 403 + 982 * laneSide,
		fs_right_x = 479 + 982 * laneSide,
		ghost_left_x = 327 + 982 * laneSide,
		keybeam_h = 853,
		gauge_x = 50 + 1820 * laneSide,
		gauge_y = 74,
		gauge_num_x = 665 + 399 * laneSide,
		gauge_num_y = 124,
		level_y = 24,
		score_x = 150 + 1400 * laneSide,
		score_label_x = 53 + 1740 * laneSide,
		label_y = 20,
		bpm_x = 1666 - 1628 * laneSide,
		bpm_y = 103,
		bpm_range_y = 60,
		rate_x = 1652 - 1540 * laneSide,
		rate_y = 42,
		exscore_y = 886,
		target_y = 846,
		graph_x = 901 - 138 * laneSide,
		rank_y = 86,
		stats_x = 1196 - 888 * laneSide,
		stats_y = 19,
		line_max_y = 827,
		progress_bar_x = 35 + 1848 * laneSide,
		progress_bar_h = 850,
		progress_x = 30 + 1848 * laneSide,
		progress_y = 1026,
		progress_w = 12,
		progress_h = 24,
		title_x = 1410 - 900 * laneSide,
		title_y = 1022,
		subtitle_y = 990,
		bga_x = 1201 - 1158 * laneSide,
		bga_y = 253,
		bga_w = 676,
		bga_h = 676,
	}

	for i = 1, 6 do
		geometry.note_size[i] = geometry.note_h
	end
	if is1p() then
		geometry.note_relative_x[1] = 144
		geometry.lane_center_relative_x[1] = 189
		for i = 2, 5 do
			geometry.lane_center_relative_x[i] = geometry.lane_center_relative_x[i-1] + (geometry.note_b_w + geometry.note_w_w) / 2 + 4
			if i % 2 == 0 then
				geometry.note_relative_x[i] = geometry.note_relative_x[i-1] + geometry.note_w_w + 4
			else
				geometry.note_relative_x[i] = geometry.note_relative_x[i-1] + geometry.note_b_w + 4
			end
		end
		geometry.note_relative_x[6] = 0
		geometry.lane_center_relative_x[6] = geometry.note_s_w / 2
	else
		geometry.note_relative_x[1] = 0
		geometry.lane_center_relative_x[1] = geometry.note_w_w / 2
		for i = 2, 6 do
			geometry.lane_center_relative_x[i] = geometry.lane_center_relative_x[i-1] + (geometry.note_b_w + geometry.note_w_w) / 2 + 4
			if i % 2 == 0 then
				geometry.note_relative_x[i] = geometry.note_relative_x[i-1] + geometry.note_w_w + 4
			else
				geometry.note_relative_x[i] = geometry.note_relative_x[i-1] + geometry.note_b_w + 4
			end
		end
		geometry.lane_center_relative_x[6] = geometry.lane_center_relative_x[6] + geometry.note_s_w / 2 - geometry.note_b_w / 2
	end

	skin.source = {
		{id = "src-bg", path = "Background/*.png"},
		{id = "src-notes", path = "Parts/Notes/*.png"},
		{id = "src-keybeam", path = "Parts/Key Beam/*.png"},
		{id = "src-judge", path = "Parts/Judge/*.png"},
		{id = "src-judgeline", path = "Parts/Judge Line/*.png"},
		{id = "src-bomb", path = "Parts/Bomb/*.png"},
		{id = "src-gauge", path = "Parts/Gauge/*.png"},
		{id = "src-lanecover", path = "Parts/Lane Cover/*.png"},
		{id = "src-liftcover", path = "Parts/Lift Cover/*.png"},
		{id = "src-number", path = "Parts/System/Number.png"},
		{id = "src-text", path = "Parts/System/Text.png"},
		{id = "src-frame", path = "Parts/System/Frame.png"},
		{id = "src-fail", path = "Parts/System/Fail.png"},
		{id = "src-fc", path = "Parts/System/FC.png"},
		{id = "src-rank", path = "Parts/System/Rank.png"},
		{id = "src-5k-lane", path = "Parts/System/5k-Lane.png"},
	}

	skin.font = {
		{id = 0, path = "Fonts/SarasaUiJ-SemiBold.ttf"},
		{id = 1, path = "Fonts/SarasaUiJ-Bold.ttf"},
	}

    skin.image = {
		{id = "bg", src = "src-bg", x = 0, y = 0, w = -1, h = -1},
		{id = "frame", src = "src-frame", x = 0, y = 0, w = 1920, h = 1080},
		{id = "lane-frame", src = "src-frame", x = 0, y = 1080, w = 880, h = 900},
		{id = "lane-bg-1p", src = "src-frame", x = 0, y = 2050, w = 768, h = 20},
		{id = "lane-bg-2p", src = "src-frame", x = 0, y = 2100, w = 768, h = 20},
		{id = "loading-bg", src = "src-frame", x = 900, y = 1150, w = 1000, h = 880},
		{id = "gauge-bg", src = "src-frame", x = 900, y = 2050, w = 804, h = 38},
		
		{id = "note-w", src = "src-notes", x = 140, y = 0, w = geometry.note_w_w, h = geometry.note_h},
		{id = "lns-w", src = "src-notes", x = 140, y = 60, w = geometry.note_w_w, h = geometry.note_h},
		{id = "lne-w", src = "src-notes", x = 140, y = 30, w = geometry.note_w_w, h = geometry.note_h},
		{id = "lnb-w", src = "src-notes", x = 140, y = 120, w = geometry.note_w_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "lna-w", src = "src-notes", x = 140, y = 90, w = geometry.note_w_w, h = geometry.note_h},
		{id = "hcns-w", src = "src-notes", x = 140, y = 210, w = geometry.note_w_w, h = geometry.note_h},
		{id = "hcne-w", src = "src-notes", x = 140, y = 180, w = geometry.note_w_w, h = geometry.note_h},
		{id = "hcnb-w", src = "src-notes", x = 140, y = 270, w = geometry.note_w_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "hcna-w", src = "src-notes", x = 140, y = 240, w = geometry.note_w_w, h = geometry.note_h},
		{id = "hcnr-w", src = "src-notes", x = 140, y = 330, w = geometry.note_w_w, h = geometry.note_h * 2, divy = 2, cycle = 100},
		{id = "hcnd-w", src = "src-notes", x = 140, y = 240, w = geometry.note_w_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "mine-w", src = "src-notes", x = 140, y = 390, w = geometry.note_w_w, h = geometry.note_h},

		{id = "note-b", src = "src-notes", x = 230, y = 0, w = geometry.note_b_w, h = geometry.note_h},
		{id = "lns-b", src = "src-notes", x = 230, y = 60, w = geometry.note_b_w, h = geometry.note_h},
		{id = "lne-b", src = "src-notes", x = 230, y = 30, w = geometry.note_b_w, h = geometry.note_h},
		{id = "lnb-b", src = "src-notes", x = 230, y = 120, w = geometry.note_b_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "lna-b", src = "src-notes", x = 230, y = 90, w = geometry.note_b_w, h = geometry.note_h},
		{id = "hcns-b", src = "src-notes", x = 230, y = 210, w = geometry.note_b_w, h = geometry.note_h},
		{id = "hcne-b", src = "src-notes", x = 230, y = 180, w = geometry.note_b_w, h = geometry.note_h},
		{id = "hcnb-b", src = "src-notes", x = 230, y = 270, w = geometry.note_b_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "hcna-b", src = "src-notes", x = 230, y = 240, w = geometry.note_b_w, h = geometry.note_h},
		{id = "hcnr-b", src = "src-notes", x = 230, y = 330, w = geometry.note_b_w, h = geometry.note_h * 2, divy = 2, cycle = 100},
		{id = "hcnd-b", src = "src-notes", x = 230, y = 240, w = geometry.note_b_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "mine-b", src = "src-notes", x = 230, y = 390, w = geometry.note_b_w, h = geometry.note_h},

		{id = "note-s", src = "src-notes", x = 0, y = 0, w = geometry.note_s_w, h = geometry.note_h},
		{id = "lns-s", src = "src-notes", x = 0, y = 60, w = geometry.note_s_w, h = geometry.note_h},
		{id = "lne-s", src = "src-notes", x = 0, y = 30, w = geometry.note_s_w, h = geometry.note_h},
		{id = "lnb-s", src = "src-notes", x = 0, y = 120, w = geometry.note_s_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "lna-s", src = "src-notes", x = 0, y = 90, w = geometry.note_s_w, h = geometry.note_h},
		{id = "hcns-s", src = "src-notes", x = 0, y = 210, w = geometry.note_s_w, h = geometry.note_h},
		{id = "hcne-s", src = "src-notes", x = 0, y = 180, w = geometry.note_s_w, h = geometry.note_h},
		{id = "hcnb-s", src = "src-notes", x = 0, y = 270, w = geometry.note_s_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "hcna-s", src = "src-notes", x = 0, y = 240, w = geometry.note_s_w, h = geometry.note_h},
		{id = "hcnr-s", src = "src-notes", x = 0, y = 330, w = geometry.note_s_w, h = geometry.note_h * 2, divy = 2, cycle = 100},
		{id = "hcnd-s", src = "src-notes", x = 0, y = 240, w = geometry.note_s_w, h = geometry.note_h * 2, divy = 2, cycle = 266},
		{id = "mine-s", src = "src-notes", x = 0, y = 390, w = geometry.note_s_w, h = geometry.note_h},

		{id = "keybeam-w", src = "src-keybeam", x = 0, y = 0, w = 50, h = 853},
		{id = "keybeam-b", src = "src-keybeam", x = 50, y = 0, w = 50, h = 853},
		{id = "keybeam-s", src = "src-keybeam", x = 100, y = 0, w = 50, h = 853},

		{id = "section-line", src = "src-frame", x = 0, y = 2160, w = 1, h = 1},
		{id = "judge-line", src = "src-judgeline", x = 0, y = 0, w = 776, h = 15},

		{id = "gauge-r1", src = "src-gauge", x = 64, y = 0, w = 16, h = 34},
		{id = "gauge-r2", src = "src-gauge", x = 80, y = 0, w = 16, h = 34},
		{id = "gauge-r3", src = "src-gauge", x = 64, y = 0, w = 16, h = 34},

		{id = "gauge-b1", src = "src-gauge", x = 96, y = 0, w = 16, h = 34},
		{id = "gauge-b2", src = "src-gauge", x = 112, y = 0, w = 16, h = 34},
		{id = "gauge-b3", src = "src-gauge", x = 96, y = 0, w = 16, h = 34},

		{id = "gauge-y1", src = "src-gauge", x = 32, y = 0, w = 16, h = 34},
		{id = "gauge-y2", src = "src-gauge", x = 48, y = 0, w = 16, h = 34},
		{id = "gauge-y3", src = "src-gauge", x = 32, y = 0, w = 16, h = 34},

		{id = "gauge-p1", src = "src-gauge", x = 160, y = 0, w = 16, h = 34},
		{id = "gauge-p2", src = "src-gauge", x = 176, y = 0, w = 16, h = 34},
		{id = "gauge-p3", src = "src-gauge", x = 160, y = 0, w = 16, h = 34},

		{id = "gauge-g1", src = "src-gauge", x = 128, y = 0, w = 16, h = 34},
		{id = "gauge-g2", src = "src-gauge", x = 144, y = 0, w = 16, h = 34},
		{id = "gauge-g3", src = "src-gauge", x = 128, y = 0, w = 16, h = 34},

		{id = "gauge-w1", src = "src-gauge", x = 0, y = 0, w = 16, h = 34},
		{id = "gauge-w2", src = "src-gauge", x = 16, y = 0, w = 16, h = 34},
		{id = "gauge-w3", src = "src-gauge", x = 0, y = 0, w = 16, h = 34},

		{id = "judge-pg", src = "src-judge", x = 0, y = 0, w = 294, h = 240, divy = 3, cycle = 99},
		{id = "judge-gr", src = "src-judge", x = 0, y = 240, w = 294, h = 160, divy = 2, cycle = 66},
		{id = "judge-gd", src = "src-judge", x = 0, y = 400, w = 588, h = 80, divx = 2, cycle = 66},
		{id = "judge-bd", src = "src-judge", x = 0, y = 480, w = 588, h = 80, divx = 2, cycle = 66},
		{id = "judge-pr", src = "src-judge", x = 0, y = 560, w = 588, h = 80, divx = 2, cycle = 66},
		{id = "judge-ms", src = "src-judge", x = 0, y = 640, w = 588, h = 80, divx = 2, cycle = 66},

		{id = "percent", src = "src-number", x = 242, y = 46, w = 23, h = 27},

		{id = "stats-label-1", src = "src-text", x = 0, y = 0, w = 140, h = 143},
		{id = "stats-label-2", src = "src-text", x = 150, y = 0, w = 140, h = 143},

		{id = "stats-aaa", src = "src-rank", x = 0, y = 10, w = 160, h = 46},
		{id = "stats-aa", src = "src-rank", x = 0, y = 60, w = 160, h = 46},
		{id = "stats-a", src = "src-rank", x = 0, y = 110, w = 160, h = 46},
		{id = "stats-b", src = "src-rank", x = 0, y = 160, w = 160, h = 46},
		{id = "stats-c", src = "src-rank", x = 0, y = 210, w = 160, h = 46},
		{id = "stats-d", src = "src-rank", x = 0, y = 260, w = 160, h = 46},
		{id = "stats-e", src = "src-rank", x = 0, y = 210, w = 160, h = 46},
		{id = "stats-f", src = "src-rank", x = 0, y = 360, w = 160, h = 46},

		{id = "auto-play", src = "src-text", x = 100, y = 190, w = 190, h = 27},
		{id = "judge-fast", src = "src-text", x = 100, y = 230, w = 102, h = 29},
		{id = "judge-slow", src = "src-text", x = 100, y = 270, w = 102, h = 29},

		{id = "unknown", src = "src-text", x = 300, y = 150, w = 150, h = 23},
		{id = "beginner", src = "src-text", x = 300, y = 190, w = 150, h = 23},
		{id = "normal", src = "src-text", x = 300, y = 230, w = 150, h = 23},
		{id = "hyper", src = "src-text", x = 300, y = 270, w = 150, h = 23},
		{id = "another", src = "src-text", x = 300, y = 310, w = 150, h = 23},
		{id = "insane", src = "src-text", x = 300, y = 350, w = 150, h = 23},
		{id = "level-label", src = "src-text", x = 300, y = 390, w = 150, h = 23},

		{id = "judge-ve", src = "src-text", x = 460, y = 150, w = 40, h = 23},
		{id = "judge-e", src = "src-text", x = 460, y = 190, w = 40, h = 23},
		{id = "judge-n", src = "src-text", x = 460, y = 230, w = 40, h = 23},
		{id = "judge-h", src = "src-text", x = 460, y = 270, w = 40, h = 23},
		{id = "judge-vh", src = "src-text", x = 460, y = 310, w = 40, h = 23},
		
		{id = "bpm-label", src = "src-text", x = 0, y = 190, w = 75, h = 20},
		{id = "score-label", src = "src-text", x = 0, y = 210, w = 75, h = 20},
		{id = "you-label", src = "src-text", x = 0, y = 230, w = 90, h = 20},
		{id = "target-label", src = "src-text", x = 0, y = 250, w = 90, h = 20},
		{id = "short-you-label", src = "src-text", x = 0, y = 270, w = 90, h = 20},
		{id = "short-target-label", src = "src-text", x = 0, y = 290, w = 90, h = 20},
		{id = "gauge-label", src = "src-text", x = 0, y = 310, w = 174, h = 20},
		{id = "hs-label", src = "src-text", x = 0, y = 330, w = 110, h = 19},

		{id = "bomb", src = "src-bomb", x = 0, y = 0, w = -1, h = -1},
		{id = "ln-bomb", src = "src-bomb", x = 0, y = 0, w = -1, h = -1},

		{id = "fail", src = "src-fail", x = 0, y = 0, w = 1920, h = 512},
		{id = "fc", src = "src-fc", x = 0, y = 0, w = 768, h = 256},

		{id = "5k-lane", src = "src-5k-lane", x = 0, y = 0, w = -1, h = -1},
	}

	for i = 1, 5, 1 do
		table.insert(skin.image, {id = "bomb-"..i, src = "src-bomb", x = 0, y = 0, w = 3072, h = 192, divx = 16, timer = 50 + i, cycle = 161})
		table.insert(skin.image, {id = "ln-bomb-"..i, src = "src-bomb", x = 0, y = 192, w = 1536, h = 192, divx = 8, timer = 70 + i, cycle = 161})
	end
	table.insert(skin.image, {id = "bomb-s", src = "src-bomb", x = 0, y = 0, w = 3072, h = 192, divx = 16, timer = 50, cycle = 161})
	table.insert(skin.image, {id = "ln-bomb-s", src = "src-bomb", x = 0, y = 192, w = 1536, h = 192, divx = 8, timer = 70, cycle = 161})

	skin.imageset = {}

    skin.value = {
		{id = "gauge-num", src = "src-number", x = 0, y = 0, w = 360, h = 46, divx = 10, digit = 3, ref = 107},
		{id = "gauge-dnum", src = "src-number", x = 0, y = 0, w = 360, h = 46, divx = 10, digit = 1, ref = 407},

		{id = "time-min", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 2, ref = 163},
		{id = "time-sec", src = "src-number", x = 0, y = 73, w = 180, h = 23, divx = 10, digit = 2, padding = 1, ref = 164},

		{id = "level-num", src = "src-number", x = 0, y = 73, w = 180, h = 23, divx = 10, digit = 4, align = 1, ref = 96},

		{id = "hs-num", src = "src-number", x = 0, y = 73, w = 180, h = 23, divx = 10, digit = 1, ref = 310},
		{id = "hs-dnum", src = "src-number", x = 0, y = 73, w = 180, h = 23, divx = 10, digit = 2, padding = 1, ref = 311},

		{id = "judge-num-pg", src = "src-judge", x = 300, y = 0, w = 600, h = 240, divx = 10, divy = 3, digit = 6, ref = 75, cycle = 99},
		{id = "judge-num-gr", src = "src-judge", x = 300, y = 240, w = 600, h = 160, divx = 10, divy = 2, digit = 6, ref = 75, cycle = 66},
		{id = "judge-num-gd", src = "src-judge", x = 300, y = 240, w = 600, h = 160, divx = 10, divy = 2, digit = 6, ref = 75, cycle = 66},
		{id = "judge-num-bd", src = "src-judge", x = 300, y = 240, w = 600, h = 160, divx = 10, divy = 2, digit = 6, ref = 75, cycle = 66},
		{id = "judge-num-pr", src = "src-judge", x = 300, y = 240, w = 600, h = 160, divx = 10, divy = 2, digit = 6, ref = 75, cycle = 66},
		{id = "judge-num-ms", src = "src-judge", x = 300, y = 240, w = 600, h = 160, divx = 10, divy = 2, digit = 6, ref = 75, cycle = 66},

		{id = "score-num", src = "src-number", x = 0, y = 0, w = 396, h = 46, divx = 11, digit = 6, ref = 100},
		{id = "ex-score-5d", src = "src-number", x = 0, y = 0, w = 396, h = 46, divx = 11, digit = 5, value = function() return math.max(main_state.number(71), 0) end},
		{id = "ex-score", src = "src-number", x = 0, y = 0, w = 396, h = 46, divx = 11, digit = 4, value = function() return math.max(main_state.number(71), 0) end},
		{id = "target-score", src = "src-number", x = 0, y = 46, w = 242, h = 27, divx = 11, digit = 4, ref = 151},
		{id = "target-score-5d", src = "src-number", x = 0, y = 46, w = 242, h = 27, divx = 11, digit = 5, ref = 151},

		{id = "rate-num", src = "src-number", x = 0, y = 46, w = 220, h = 27, divx = 10, digit = 3, value = function() return math.max(main_state.number(102), 0) end},
		{id = "rate-dnum", src = "src-number", x = 0, y = 46, w = 220, h = 27, divx = 10, digit = 2, padding = 1, value = function() return math.max(main_state.number(103), 0) end},
		{id = "bpm-main", src = "src-number", x = 0, y = 0, w = 360, h = 46, divx = 10, digit = 6, ref = 160, align = 2},
		{id = "bpm-main-s", src = "src-number", x = 0, y = 46, w = 220, h = 27, divx = 10, digit = 10, ref = 160, align = 2},
		{id = "bpm-min", src = "src-number", x = 0, y = 46, w = 242, h = 27, divx = 11, digit = 3, ref = 91, align = 2},
		{id = "bpm-max", src = "src-number", x = 0, y = 46, w = 242, h = 27, divx = 11, digit = 3, ref = 90, align = 2},
		
		{id = "ghost-target", src = "src-number", x = 0, y = 96, w = 264, h = 58, divx = 12, divy = 2, digit = 6, ref = 108, align = 2},
		{id = "ghost-best", src = "src-number", x = 0, y = 96, w = 264, h = 58, divx = 12, divy = 2, digit = 6, ref = 152, align = 2},

		{id = "lanecover-white-num", src = "src-number", x = 0, y = 96, w = 220, h = 29, divx = 10, digit = 4, ref = 14, align = 2},
		{id = "lift-white-num", src = "src-number", x = 0, y = 96, w = 220, h = 29, divx = 10, digit = 4, ref = 314, align = 2},
		{id = "green-num", src = "src-number", x = 0, y = 96, w = 220, h = 29, divx = 10, digit = 4, ref = 313, align = 2},

		{id = "pg-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 110},
		{id = "gr-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 111},
		{id = "gd-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 112},
		{id = "bd-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 113},
		{id = "pr-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 114},
		{id = "fast-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 423},
		{id = "slow-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 424},
		{id = "cb-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 425},
		{id = "ep-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 420},
		{id = "maxc-num", src = "src-number", x = 0, y = 73, w = 198, h = 23, divx = 11, digit = 4, ref = 105},
	}

	skin.text = {
		{id = "title", font = 1, size = 36, overflow = 1, align = 1, ref = 10},
		{id = "artist", font = 0, size = 24, overflow = 1, align = 1, ref = 14},
		{id = "table", font = 0, size = 24, overflow = 1, align = 1, ref = 1003},
		{id = "loading-title", font = 1, size = 80, overflow = 1, align = 1, ref = 10},
		{id = "loading-artist", font = 0, size = 40, overflow = 1, align = 1, ref = 14},
		{id = "loading-genre", font = 0, size = 40, overflow = 1, align = 1, ref = 13},
	}

	skin.slider = {
		{id = "song-progress", src = "src-frame", x = 0, y = 2000, w = geometry.progress_w, h = geometry.progress_h, angle = 2, range = geometry.progress_bar_h - geometry.progress_h, type = 6},
		{id = "lanecover", src = "src-lanecover", x = 0, y = 0, w = 768, h = 880, angle = 2, range = 880, type = 4},
	}

	skin.hiddenCover = {}

	skin.liftCover = {
		{id = "liftcover", src = "src-liftcover", x = 0, y = 0, w = 768, h = 880, disapearLine = geometry.lane_y},
	}

	skin.graph = {
		{id = "graph-you", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 111},
		{id = "graph-you-now", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 110},
		{id = "graph-best", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 113},
		{id = "graph-best-now", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 112},
		{id = "graph-target", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 115},
		{id = "graph-target-now", src = "src-frame", x = 0, y = 2160, w = 1, h = 1, type = 114},
		{id = 'loading-bar', src = "src-frame", x = 0, y = 2160, w = 1, h = 1, angle = 2, type = 102}
	}

-- Notes Prepare

	skin.note = {
		id = "notes",
		note = {"note-w", "note-b", "note-w", "note-b", "note-w", "note-s"},
		lnend = {"lne-w", "lne-b", "lne-w", "lne-b", "lne-w", "lne-s"},
		lnstart = {"lns-w", "lns-b", "lns-w", "lns-b", "lns-w", "lns-s"},
		lnbody = {"lnb-w", "lnb-b", "lnb-w", "lnb-b", "lnb-w", "lnb-s"},
		lnactive = {"lna-w", "lna-b", "lna-w", "lna-b", "lna-w", "lna-s"},
		hcnend = {"hcne-w", "hcne-b", "hcne-w", "hcne-b", "hcne-w", "hcne-s"},
		hcnstart = {"hcns-w", "hcns-b", "hcns-w", "hcns-b", "hcns-w", "hcns-s"},
		hcnbody = {"hcnb-w", "hcnb-b", "hcnb-w", "hcnb-b", "hcnb-w", "hcnb-s"},
		hcnactive = {"hcna-w", "hcna-b", "hcna-w", "hcna-b", "hcna-w", "hcna-s"},
		hcndamage = {"hcnd-w", "hcnd-b", "hcnd-w", "hcnd-b", "hcnd-w", "hcnd-s"},
		hcnreactive = {"hcnr-w", "hcnr-b", "hcnr-w", "hcnr-b", "hcnr-w", "hcnr-s"},
		mine = {"mine-w", "mine-b", "mine-w", "mine-b", "mine-w", "mine-s"},
		hidden = {},
		processed = {},
		size = geometry.note_size,
		dst = {},
		group = {
			{id = "section-line", offset = 3, op = {81}, dst = {
				{x = geometry.lane_x_available, y = geometry.lane_y, w = geometry.lane_w_available, h = 3, r = 127, g = 127, b = 127}
			}}
		},
		time = {
			{id = "section-line", offset = 3, op = {81}, dst = {
				{x = geometry.lane_x_available, y = geometry.lane_y, w = geometry.lane_w_available, h = 3, r = 100, g = 100, b = 255}
			}}
		},
		bpm = {
			{id = "section-line", offset = 3, op = {81}, dst = {
				{x = geometry.lane_x_available, y = geometry.lane_y, w = geometry.lane_w_available, h = 3, r = 100, g = 255, b = 100}
			}}
		},
		stop = {
			{id = "section-line", offset = 3, op = {81}, dst = {
				{x = geometry.lane_x_available, y = geometry.lane_y, w = geometry.lane_w_available, h = 3, r = 255, g = 100, b = 100}
			}}
		}
	}
	for i = 1, 6 do
		if i == 6 then
			table.insert(skin.note.dst, {x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_s_w, h = geometry.lane_h})
		elseif i % 2 == 1 then
			table.insert(skin.note.dst, {x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_w_w, h = geometry.lane_h})
		else
			table.insert(skin.note.dst, {x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_b_w, h = geometry.lane_h})
		end
	end

-- Gauge Prepare

	skin.gauge = {
		id = "gauge",
		nodes = {
			"gauge-r1","gauge-p1","gauge-r2","gauge-p2","gauge-r3","gauge-p3",
			"gauge-r1","gauge-g1","gauge-r2","gauge-g2","gauge-r3","gauge-g3",
			"gauge-r1","gauge-b1","gauge-r2","gauge-b2","gauge-r3","gauge-b3",
			"gauge-r1","gauge-r1","gauge-r2","gauge-r2","gauge-r3","gauge-r3",
			"gauge-y1","gauge-y1","gauge-y2","gauge-y2","gauge-y3","gauge-y3",
			"gauge-w1","gauge-w1","gauge-w2","gauge-w2","gauge-w3","gauge-w3"
		}
	}

-- Judge Prepare

	skin.judge = {
		{
			id = "judge",
			index = 0,
			images = {
				{id = "judge-pg", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 228, y = 400, w = 294, h = 80},
					{time = 500}
				}},
				{id = "judge-gr", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 228, y = 400, w = 294, h = 80},
					{time = 500}
				}},
				{id = "judge-gd", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 228, y = 400, w = 294, h = 80},
					{time = 500}
				}},
				{id = "judge-bd", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 236, y = 400, w = 294, h = 80},
					{time = 500}
				}},
				{id = "judge-pr", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 236, y = 400, w = 294, h = 80},
					{time = 500}
				}},
				{id = "judge-ms", loop = -1, timer = 46, offsets = {3, 32}, dst = {
					{time = 0, x = geometry.lane_x + 236, y = 400, w = 294, h = 80},
					{time = 500}
				}}
			},
			numbers = {
				{id = "judge-num-pg", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}},
				{id = "judge-num-gr", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}},
				{id = "judge-num-gd", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}},
				{id = "judge-num-bd", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}},
				{id = "judge-num-pr", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}},
				{id = "judge-num-ms", loop = -1, timer = 46, offsets = {32},  dst = {
					{time = 0, x = 315, y = 0, w = 60, h = 80},
					{time = 500}
				}}
			},
			shift = true
		}
	}
	
	skin.bga = {id = "bga"}
	
	skin.judgegraph = {
		{id = 'judge-graph', type = 1, backTexOff = 1}
	}
	
	skin.bpmgraph = {
		{id = "bpm-graph"}
	}
	
	skin.timingvisualizer = {}
   
	skin.destination = {}

-- Background and frame

	if isPureModeOff() then
		table.insert(skin.destination, {id = "bg", dst = {
			{x = 0, y = 0, w = 1920, h = 1080}
		}})
		if is1p() then
			table.insert(skin.destination, {id = "frame", dst = {
				{x = 0, y = 0, w = 1920, h = 1080}
			}})
		else
			table.insert(skin.destination, {id = "frame", dst = {
				{x = 1920, y = 0, w = -1920, h = 1080}
			}})
		end
	end

-- Song Info

	table.insert(skin.destination, {id = "title", filter = 1, dst = {
		{x = geometry.title_x, y = geometry.title_y, w = 900, h = 36}
	}})

	table.insert(skin.destination, {id = "artist", op = {-1008}, filter = 1, dst = {
		{x = geometry.title_x, y = geometry.subtitle_y, w = 900, h = 24}
	}})

	table.insert(skin.destination, {id = "artist", loop = 0, op = {1008}, filter = 1, dst = {
		{time = 0, x = geometry.title_x, y = geometry.subtitle_y, w = 900, h = 24, a = 0},
		{time = 250, a = 255},
		{time = 3250, a = 255},
		{time = 3500, a = 0},
		{time = 7000, a = 0},
	}})

	table.insert(skin.destination, {id = "table", loop = 0, op = {1008}, filter = 1, dst = {
		{time = 0, x = geometry.title_x, y = geometry.subtitle_y, w = 900, h = 24, a = 0},
		{time = 3500, a = 0},
		{time = 3750, a = 255},
		{time = 6750, a = 255},
		{time = 7000, a = 0},
	}})

	table.insert(skin.destination, {id = -110, dst = {
		{x = geometry.bga_x, y = geometry.bga_y, w = geometry.bga_w, h = geometry.bga_h}
	}})

-- BGA

	table.insert(skin.destination, {id = "bga", stretch = 1, dst = {
		{x = geometry.bga_x, y = geometry.bga_y, w = geometry.bga_w, h = geometry.bga_h}
	}})

	table.insert(skin.destination, {id = "judge-graph", dst = {
		{x = geometry.bga_x, y = geometry.bga_y - 28, w = geometry.bga_w, h = 28},
	}})

	table.insert(skin.destination, {id = "bpm-graph", dst = {
		{x = geometry.bga_x, y = geometry.bga_y - 28, w = geometry.bga_w, h = 28},
	}})

-- Song progress bar

	if isPureModeOff() then
		table.insert(skin.destination, {id = -111, dst = {
			{x = geometry.progress_bar_x, y = geometry.lane_y, w = 2, h = geometry.progress_bar_h, r = 160, g = 160, b = 160}
		}})
	else
		table.insert(skin.destination, {id = -111, dst = {
			{x = geometry.progress_bar_x, y = geometry.lane_y, w = 2, h = geometry.progress_bar_h, a = 127}
		}})
	end

	table.insert(skin.destination, {id = "song-progress", dst = {
		{x = geometry.progress_x, y = geometry.progress_y, w = geometry.progress_w, h = geometry.progress_h}
	}})

-- BPM

	table.insert(skin.destination, {id = "bpm-label", dst = {
		{x = geometry.bpm_x + 71, y = geometry.label_y, w = 75, h = 20}
	}})
	
	if isLargeBPM() then
		table.insert(skin.destination, {id = "bpm-main-s", dst = {
			{x = geometry.bpm_x - 2 , y = geometry.bpm_y + 10, w = 22, h = 27}
		}})
	else
		table.insert(skin.destination, {id = "bpm-main", dst = {
			{x = geometry.bpm_x, y = geometry.bpm_y, w = 36, h = 46}
		}})
	end

	table.insert(skin.destination, {id = "bpm-min", op = {177}, dst = {
		{x = geometry.bpm_x + 14, y = geometry.bpm_range_y, w = 22, h = 27}
	}})

	table.insert(skin.destination, {id = -111, op = {177}, dst = {
		{x = geometry.bpm_x + 98, y = geometry.bpm_range_y + 11, w = 20, h = 5}
	}})

	table.insert(skin.destination, {id = -111, op = {176}, dst = {
		{x = geometry.bpm_x + 68, y = geometry.bpm_range_y + 11, w = 80, h = 5, a = 127}
	}})

	table.insert(skin.destination, {id = "bpm-max", op = {177}, dst = {
		{x = geometry.bpm_x + 136, y = geometry.bpm_range_y, w = 22, h = 27}
	}})

-- Score graph area

	table.insert(skin.destination, {id = "auto-play", op = {33}, dst = {
		{x = geometry.graph_x + 34, y = geometry.rate_y, w = 190, h = 27}
	}})

	table.insert(skin.destination, {id = "rate-num", op = {32}, dst = {
		{x = geometry.graph_x + 48, y = geometry.rate_y, w = 22, h = 27}
	}})

	table.insert(skin.destination, {id = -111, op = {32}, dst = {
		{x = geometry.graph_x + 116, y = geometry.rate_y + 1, w = 5, h = 5}
	}})

	table.insert(skin.destination, {id = "rate-dnum", op = {32}, dst = {
		{x = geometry.graph_x + 123, y = geometry.rate_y, w = 22, h = 27}
	}})

	table.insert(skin.destination, {id = "percent", op = {32}, dst = {
		{x = geometry.graph_x + 167, y = geometry.rate_y, w = 22, h = 27}
	}})

	table.insert(skin.destination, {id = -111, filter = 1, dst = {
		{x = geometry.graph_x, y = geometry.line_max_y, w = 256, h = 1}
	}})

	table.insert(skin.destination, {id = -111, filter = 1, dst = {
		{x = geometry.graph_x, y = geometry.line_max_y - 75, w = 256, h = 1, a = 127}
	}})

	table.insert(skin.destination, {id = -111, filter = 1, dst = {
		{x = geometry.graph_x, y = geometry.line_max_y - 150, w = 256, h = 1, a = 127}
	}})

	table.insert(skin.destination, {id = -111, filter = 1, dst = {
		{x = geometry.graph_x, y = geometry.line_max_y - 225, w = 256, h = 1, a = 127}
	}})

	table.insert(skin.destination, {id = "graph-you", dst = {
		{x = geometry.graph_x + 150 * laneSide + 21, y = geometry.line_max_y - 675, w = 65, h = 675, a = 63}
	}})

	table.insert(skin.destination, {id = "graph-best", dst = {
		{x = geometry.graph_x + 96, y = geometry.line_max_y - 675, w = 65, h = 675, a = 63}
	}})

	table.insert(skin.destination, {id = "graph-target", dst = {
		{x = geometry.graph_x - 150 * laneSide + 171, y = geometry.line_max_y - 675, w = 65, h = 675, a = 63}
	}})

	table.insert(skin.destination, {id = "graph-you-now", dst = {
		{x = geometry.graph_x + 150 * laneSide + 21, y = geometry.line_max_y - 675, w = 65, h = 675, r = 0, g = 160, b = 233}
	}})

	table.insert(skin.destination, {id = "graph-best-now", dst = {
		{x = geometry.graph_x + 96, y = geometry.line_max_y - 675, w = 65, h = 675, r = 50, g = 177, b = 108}
	}})

	table.insert(skin.destination, {id = "graph-target-now", dst = {
		{x = geometry.graph_x - 150 * laneSide + 171, y = geometry.line_max_y - 675, w = 65, h = 675, r = 229, g = 0, b = 79}
	}})
	
	table.insert(skin.destination, {id = -111, filter = 1, dst = {
		{x = geometry.graph_x, y = geometry.line_max_y - 675, w = 256, h = 1}
	}})
	
	table.insert(skin.destination, {id = "target-label", dst = {
		{x = geometry.graph_x + 18, y = geometry.target_y + 2, w = 90, h = 20}
	}})
	
	if isLongChart() then
		table.insert(skin.destination, {id = "short-you-label", dst = {
			{x = geometry.graph_x + 18, y = geometry.exscore_y + 11, w = 90, h = 20}
		}})
		table.insert(skin.destination, {id = "ex-score-5d", dst = {
			{x = geometry.graph_x + 99 - 36, y = geometry.exscore_y, w = 36, h = 46}
		}})
		table.insert(skin.destination, {id = "target-score-5d", dst = {
			{x = geometry.graph_x + 153 - 22, y = geometry.target_y, w = 22, h = 27}
		}})
	else
		table.insert(skin.destination, {id = "you-label", dst = {
			{x = geometry.graph_x + 18, y = geometry.exscore_y + 11, w = 90, h = 20}
		}})
		table.insert(skin.destination, {id = "ex-score", dst = {
			{x = geometry.graph_x + 99, y = geometry.exscore_y, w = 36, h = 46}
		}})
		table.insert(skin.destination, {id = "target-score", dst = {
			{x = geometry.graph_x + 153, y = geometry.target_y, w = 22, h = 27}
		}})
	end

	table.insert(skin.destination, {id = "stats-aaa", op = {340}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-aa", op = {341}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-a", op = {342}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-b", op = {343}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-c", op = {344}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-d", op = {345}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-e", op = {346}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

	table.insert(skin.destination, {id = "stats-f", op = {347}, dst = {
		{x = geometry.graph_x + 49, y = geometry.rank_y, w = 160, h = 46}
	}})

-- Playing statistics

	table.insert(skin.destination, {id = "stats-label-1", dst = {
		{x = geometry.stats_x, y = geometry.stats_y, w = 140, h = 143}
	}})

	table.insert(skin.destination, {id = "stats-label-2", dst = {
		{x = geometry.stats_x + 229, y = geometry.stats_y, w = 140, h = 143}
	}})

	table.insert(skin.destination, {id = "pg-num", dst = {
		{x = geometry.stats_x + 138, y = geometry.stats_y + 120, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "gr-num", dst = {
		{x = geometry.stats_x + 138, y = geometry.stats_y + 90, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "gd-num", dst = {
		{x = geometry.stats_x + 138, y = geometry.stats_y + 60, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "bd-num", dst = {
		{x = geometry.stats_x + 138, y = geometry.stats_y + 30, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "pr-num", dst = {
		{x = geometry.stats_x + 138, y = geometry.stats_y, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "fast-num", dst = {
		{x = geometry.stats_x + 340, y = geometry.stats_y + 120, w = 18, h = 23, r = 108, g = 150, b = 255}
	}})

	table.insert(skin.destination, {id = "slow-num", dst = {
		{x = geometry.stats_x + 340, y = geometry.stats_y + 90, w = 18, h = 23, r = 255, g = 108, b = 122}
	}})

	table.insert(skin.destination, {id = "cb-num", dst = {
		{x = geometry.stats_x + 340, y = geometry.stats_y + 60, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "ep-num", dst = {
		{x = geometry.stats_x + 340, y = geometry.stats_y + 30, w = 18, h = 23}
	}})

	table.insert(skin.destination, {id = "maxc-num", dst = {
		{x = geometry.stats_x + 340, y = geometry.stats_y, w = 18, h = 23}
	}})

-- Lane background

	if isPureModeOff() then
		if is1p() then
			table.insert(skin.destination, {id = "lane-bg-1p", loop = 900, dst = {
				{time = 0, x = geometry.lane_x, y = geometry.lane_y + 900, w = geometry.lane_w, h = geometry.lane_h, r = laneBrightness, g = laneBrightness, b = laneBrightness, acc = 2},
				{time = 250},
				{time = 900, y = geometry.lane_y}
			}})
		else
			table.insert(skin.destination, {id = "lane-bg-2p", loop = 900, dst = {
				{time = 0, x = geometry.lane_x, y = geometry.lane_y + 900, w = geometry.lane_w, h = geometry.lane_h, r = laneBrightness, g = laneBrightness, b = laneBrightness, acc = 2},
				{time = 250},
				{time = 900, y = geometry.lane_y}
			}})
		end
	end

	table.insert(skin.destination, {id = "5k-lane", loop = 900, dst = {
		{time = 0, x = geometry.lane_5k_x, y = geometry.lane_y + 900, w = 174, h = geometry.lane_h, acc = 2},
		{time = 250},
		{time = 900, y = geometry.lane_y}
	}})

-- Lift Cover

	table.insert(skin.destination,
	{id = "liftcover", loop = 1150, dst = {
		{time = 0, x = geometry.lane_x, y = geometry.lane_y - geometry.lane_h - 900, w = geometry.lane_w, h = geometry.lane_h, acc = 2},
		{time = 800},
		{time = 1150, y = geometry.lane_y - geometry.lane_h}
	}})

-- Judge line
	
	table.insert(skin.destination, {id = "judge-line", loop = 1150, offset = 3, dst = {
		{time = 0, x = geometry.lane_x + 384, y = geometry.lane_y, w = 0, h = 15, acc = 2},
		{time = 900},
		{time = 1150, x = geometry.lane_x - 4, w = geometry.lane_w + 8}
	}})

-- Key beam

	for i = 1, 5, 2 do
		table.insert(skin.destination, {id = "keybeam-w", offset = 3, timer = 100 + i, op = {81}, blend = 1, dst = {
			{x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_w_w, h = geometry.keybeam_h}
		}})
		table.insert(skin.destination, {id = "keybeam-w", offset = 3, timer = 120 + i, loop = -1, op = {81}, blend = 1, dst = {
			{time = 0, x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_w_w, h = geometry.keybeam_h},
			{time = 80, x = geometry.lane_x_available + geometry.lane_center_relative_x[i], w = 0, a = 0}
		}})
	end

	for i = 2, 4, 2 do
		table.insert(skin.destination, {id = "keybeam-b", offset = 3, timer = 100 + i, op = {81}, blend = 1, dst = {
			{x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_b_w, h = geometry.keybeam_h}
		}})
		table.insert(skin.destination, {id = "keybeam-b", offset = 3, timer = 120 + i, loop = -1, op = {81}, blend = 1, dst = {
			{time = 0, x = geometry.lane_x_available + geometry.note_relative_x[i], y = geometry.lane_y, w = geometry.note_b_w, h = geometry.keybeam_h},
			{time = 80, x = geometry.lane_x_available + geometry.lane_center_relative_x[i], w = 0, a = 0}
		}})
	end

	table.insert(skin.destination,	{id = "keybeam-s", offset = 3, timer = 100, op = {81,940}, blend = 1, dst = {
		{x = geometry.lane_x_available + geometry.note_relative_x[6], y = geometry.lane_y, w = geometry.note_s_w, h = geometry.keybeam_h}
	}})

	table.insert(skin.destination, {id = "keybeam-s", offset = 3, timer = 100, op = {81,941}, loop = -1, blend = 1, dst = {
		{time = 0, x = geometry.lane_x_available + geometry.note_relative_x[6], y = geometry.lane_y, w = geometry.note_s_w, h = geometry.keybeam_h},
		{time = 80},
		{time = 160, x = geometry.lane_x_available + geometry.lane_center_relative_x[6], w = 0, a = 0}
	}})

	table.insert(skin.destination, {id = "keybeam-s", offset = 3, timer = 120, op = {81,940}, loop = -1, blend = 1, dst = {
		{time = 0, x = geometry.lane_x_available + geometry.note_relative_x[6], y = geometry.lane_y, w = geometry.note_s_w, h = geometry.keybeam_h},
		{time = 80, x = geometry.lane_x_available + geometry.lane_center_relative_x[6], w = 0, a = 0}
	}})

-- Notes
	
	table.insert(skin.destination, {id = "notes",})

-- Full combo animation

	table.insert(skin.destination, {id = "fc", loop = 2000, timer = 48, offset = 3, dst = {
		{time = 0, x = geometry.lane_x - 768, y = geometry.lane_y - 146, w = geometry.lane_w * 3, h = 768, a = 0, acc = 1},
		{time = 500, x = geometry.lane_x, y = geometry.lane_y + 110, w = geometry.lane_w, h = 256, a = 255},
		{time = 1500},
		{time = 2000, a = 0},
	}})

	table.insert(skin.destination, {id = -111, loop = 1200, timer = 48, offset = 3, dst = {
		{time = 0, x = geometry.lane_x, y = geometry.lane_y, w = geometry.lane_w, h = geometry.lane_h, a = 0, acc = 2},
		{time = 499, a = 0},
		{time = 500, a = 223},
		{time = 1200, a = 0},
	}})

	table.insert(skin.destination, {id = "fc", loop = 1500, timer = 48, offset = 3, dst = {
		{time = 0, x = geometry.lane_x, y = geometry.lane_y + 110, w = geometry.lane_w, h = 256, a = 0},
		{time = 499, a = 0},
		{time = 500, x = geometry.lane_x, y = geometry.lane_y + 110, w = geometry.lane_w, h = 256, a = 255},
		{time = 1500, x = geometry.lane_x - 120, y = geometry.lane_y + 70, w = geometry.lane_w + 240, h = 336, a = 0},
	}})

-- Lane cover

	table.insert(skin.destination, {id = "lanecover", loop = 1150, dst = {
		{time = 0, x = geometry.lane_x, y = 1980, w = geometry.lane_w, h = geometry.lane_h, acc = 2},
		{time = 500},
		{time = 1150, y = 1080}
	}})

	table.insert(skin.destination, {id = "lanecover-white-num", loop = 1150, offset = 4, op = {270}, dst = {
		{time = 0, x = geometry.lane_x + 114, y = 1988, w = 22, h = 29, acc = 2},
		{time = 500},
		{time = 1150, y = 1088}
	}})

	table.insert(skin.destination, {id = "green-num", loop = 1150, offset = 4, op = {270}, dst = {
		{time = 0, x = geometry.lane_x + 566, y = 1988, w = 22, h = 29, r = 64, g = 255, b = 96, acc = 2},
		{time = 500},
		{time = 1150, y = 1088}
	}})

-- Lane frame

	if isPureModeOff() then
		if is1p() then
			table.insert(skin.destination, {id = "lane-frame", loop = 900, dst = {
				{time = 0, x = 0, y = 1080, w = 880, h = 900, acc = 2},
				{time = 250},
				{time = 900, y = 180}
			}})
		else
			table.insert(skin.destination, {id = "lane-frame", loop = 900, dst = {
				{time = 0, x = 1920, y = 1080, w = -880, h = 900, acc = 2},
				{time = 250},
				{time = 900, y = 180}
			}})
		end
	end

-- Judge details

	table.insert(skin.destination, {id = "ghost-target", loop = -1, timer = 46, op = {912,920}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.ghost_center_x, y = geometry.judge_y + 100, w = 22, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "ghost-target", loop = -1, timer = 46, op = {912,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.ghost_left_x, y = geometry.judge_y + 100, w = 22, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "ghost-best", loop = -1, timer = 46, op = {911,920}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.ghost_center_x, y = geometry.judge_y + 100, w = 22, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "ghost-best", loop = -1, timer = 46, op = {911,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.ghost_left_x, y = geometry.judge_y + 100, w = 22, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "judge-fast", loop = -1, timer = 46, op = {1242,910,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.fs_center_x, y = geometry.judge_y + 100, w = 102, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "judge-fast", loop = -1, timer = 46, op = {1242,-910,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.fs_right_x, y = geometry.judge_y + 100, w = 102, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "judge-slow", loop = -1, timer = 46, op = {1243,910,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.fs_center_x, y = geometry.judge_y + 100, w = 102, h = 29},
		{time = 500}
	}})

	table.insert(skin.destination, {id = "judge-slow", loop = -1, timer = 46, op = {1243,-910,921}, offsets = {3,33}, dst = {
		{time = 0, x = geometry.fs_right_x, y = geometry.judge_y + 100, w = 102, h = 29},
		{time = 500}
	}})

-- Judge

	table.insert(skin.destination, {id = "judge",})

-- Difficulty label

	table.insert(skin.destination, {id = "unknown", loop = 1150, op = {150}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "beginner", loop = 1150, op = {151}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "normal", loop = 1150, op = {152}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "hyper", loop = 1150, op = {153}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "another", loop = 1150, op = {154}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "insane", loop = 1150, op = {155}, dst = {
		{time = 900, x = geometry.gauge_x + 50 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

-- Level label

	table.insert(skin.destination, {id = "level-label", loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 215 - 800 * laneSide, y = geometry.level_y, w = 150, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "level-num", loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 260 - 800 * laneSide, y = geometry.level_y, w = 18, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

-- Judge level label

	table.insert(skin.destination, {id = "judge-ve", loop = 1150, op = {184}, dst = {
		{time = 900, x = geometry.gauge_x + 380 - 800 * laneSide, y = geometry.level_y, w = 40, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "judge-e", loop = 1150, op = {183}, dst = {
		{time = 900, x = geometry.gauge_x + 380 - 800 * laneSide, y = geometry.level_y, w = 40, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "judge-n", loop = 1150, op = {182}, dst = {
		{time = 900, x = geometry.gauge_x + 380 - 800 * laneSide, y = geometry.level_y, w = 40, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "judge-h", loop = 1150, op = {181}, dst = {
		{time = 900, x = geometry.gauge_x + 380 - 800 * laneSide, y = geometry.level_y, w = 40, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "judge-vh", loop = 1150, op = {180}, dst = {
		{time = 900, x = geometry.gauge_x + 380 - 800 * laneSide, y = geometry.level_y, w = 40, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "hs-label", loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 550 - 800 * laneSide, y = geometry.level_y, w = 110, h = 19, a = 0},
		{time = 1150, a = 255},
	}})

-- Hi-speed label

	table.insert(skin.destination, {id = "hs-num", loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 689 - 800 * laneSide, y = geometry.level_y, w = 18, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = -111, loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 709 - 800 * laneSide, y = geometry.level_y + 1, w = 4, h = 4, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "hs-dnum", loop = 1150, dst = {
		{time = 900, x = geometry.gauge_x + 714 - 800 * laneSide, y = geometry.level_y, w = 18, h = 23, a = 0},
		{time = 1150, a = 255},
	}})

	table.insert(skin.destination, {id = "time-min", dst = {
		{x = geometry.graph_x + 2 + 170 * laneSide, y = 3, w = 18, h = 23},
	}})

-- Remaining time

	table.insert(skin.destination, {id = -111, dst = {
		{x = geometry.graph_x + 41 + 170 * laneSide, y = 6, w = 4, h = 4},
	}})

	table.insert(skin.destination, {id = -111, dst = {
		{x = geometry.graph_x + 41 + 170 * laneSide, y = 18, w = 4, h = 4},
	}})

	table.insert(skin.destination, {id = "time-sec", dst = {
		{x = geometry.graph_x + 47 + 170 * laneSide, y = 3, w = 18, h = 23},
	}})

-- Score

	table.insert(skin.destination, {id = "score-label", loop = 1150, dst = {
		{time = 900, x = geometry.score_label_x, y = geometry.gauge_num_y, w = 75, h = 20, a = 0},
		{time = 1150, a = 255},
	}})
	
	if isLongChart() then
		table.insert(skin.destination, {id = "ex-score-5d", loop = 1150, dst = {
			{time = 900, x = geometry.score_x + 18, y = geometry.gauge_num_y, w = 36, h = 46, a = 0},
			{time = 1150, a = 255},
		}})
	else
		table.insert(skin.destination, {id = "ex-score", loop = 1150, dst = {
			{time = 900, x = geometry.score_x + 36, y = geometry.gauge_num_y, w = 36, h = 46, a = 0},
			{time = 1150, a = 255},
		}})
	end

-- Groove gauge

	if isPureModeOff() then
		table.insert(skin.destination, {id = "gauge-bg", loop = 900, dst = {
			{time = 250, x = geometry.gauge_x - 902 + 1804 * laneSide, y = geometry.gauge_y - 2, w = 804 * (1 - 2 * laneSide), h = 38, acc = 2},
			{time = 900, x = geometry.gauge_x - 2 + 4 * laneSide},
		}})
	end

	table.insert(skin.destination, {id = "gauge-label", loop = 900, dst = {
		{time = 250, x = geometry.gauge_x - 500 + 826 * laneSide, y = geometry.gauge_num_y, w = 174, h = 20, acc = 2},
		{time = 900, x = geometry.gauge_x + 400 - 974 * laneSide},
	}})
	
	table.insert(skin.destination, {id = "gauge", loop = 900, dst = {
		{time = 250, x = geometry.gauge_x - 900 + 1800 * laneSide, y = geometry.gauge_y, w = 800 * (1 - 2 * laneSide), h = 34, acc = 2},
		{time = 900, x = geometry.gauge_x},
	}})
	
	table.insert(skin.destination, {id = "gauge-num", loop = 900, dst = {
		{time = 250, x = geometry.gauge_num_x - 900 + 1800 * laneSide, y = geometry.gauge_num_y, w = 36, h = 46, acc = 2},
		{time = 900,x = geometry.gauge_num_x},
	}})
	
	table.insert(skin.destination, {id = -111, loop = 900, dst = {
		{time = 250, x = geometry.gauge_num_x - 789 + 1800 * laneSide, y = geometry.gauge_num_y + 2, w = 9, h = 9, acc = 2},
		{time = 900, x = geometry.gauge_num_x + 111},
	}})
	
	table.insert(skin.destination, {id = "gauge-dnum", loop = 900, dst = {
		{time = 250, x = geometry.gauge_num_x - 777 + 1800 * laneSide, y = geometry.gauge_num_y, w = 36, h = 46, acc = 2},
		{time = 900, x = geometry.gauge_num_x + 123},
	}})

	table.insert(skin.destination, {id = "percent", loop = 900, dst = {
		{time = 250, x = geometry.gauge_num_x - 739 + 1800 * laneSide, y = geometry.gauge_num_y + 1, w = 23, h = 27, acc = 2},
		{time = 900, x = geometry.gauge_num_x + 161},
	}})

-- Lift cover

	table.insert(skin.destination, {id = "lift-white-num", loop = 1150, offset = 3, op = {270, 272}, dst = {
		{time = 0, x = geometry.lane_x + 114, y = geometry.lane_y - 938, w = 22, h = 29, acc = 2},
		{time = 800},
		{time = 1150, y = geometry.lane_y - 38}
	}})

	table.insert(skin.destination, {id = "green-num", loop = 1150, offset = 3, op = {270, 272}, dst = {
		{time = 0, x = geometry.lane_x + 566, y = geometry.lane_y - 938, w = 22, h = 29, r = 64, g = 255, b = 96, acc = 2},
		{time = 800},
		{time = 1150, y = geometry.lane_y - 38}
	}})

-- Bomb

	table.insert(skin.destination, {id = "bomb", dst = {
		{x = 0, y = 0, w = 1, h = 1, a = 1}
	}})

	table.insert(skin.destination, {id = "ln-bomb", dst = {
		{x = 0, y = 0, w = 1, h = 1, a = 1}
	}})

	for i = 1, 5, 1 do
		table.insert(skin.destination, {id = "bomb-"..i, offset = 3, loop = -1, filter = 1, timer = 50 + i, blend = 2, dst = {
			{time = 0, x = geometry.lane_x_available + geometry.lane_center_relative_x[i] - 144, y = geometry.lane_y - 136, w = 288, h = 288},
			{time = 160}
		}})
	end

	table.insert(skin.destination, {id = "bomb-s", offset = 3, loop = -1, filter = 1, timer = 50, blend = 2, dst = {
		{time = 0, x = geometry.lane_x_available + geometry.lane_center_relative_x[6] - 144, y = geometry.lane_y - 136, w = 288, h = 288},
		{time = 160}
	}})

	for i = 1, 5, 1 do
		table.insert(skin.destination, {id = "ln-bomb-"..i, offset = 3, filter = 1, timer = 70 + i, blend = 2, dst = {
			{time = 0, x = geometry.lane_x_available + geometry.lane_center_relative_x[i] - 144, y = geometry.lane_y - 136, w = 288, h = 288},
			{time = 160}
		}})
	end

	table.insert(skin.destination, {id = "ln-bomb-s", offset = 3, filter = 1, timer = 70, blend = 2, dst = {
		{time = 0, x = geometry.lane_x_available + geometry.lane_center_relative_x[6] - 144, y = geometry.lane_y - 136, w = 288, h = 288},
		{time = 160}
	}})

	table.insert(skin.destination, {id = -110, loop = 250, dst = {
		{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 255},
		{time = 250, a = 0},
	}})

	table.insert(skin.destination, {id = "loading-bg", loop = 500, op = {80}, dst = {
		{time = 0, x = 460, y = 100, w = 1000, h = 880, a = 0},
		{time = 250},
		{time = 500, a = 255},
	}})

	table.insert(skin.destination, {id = "loading-bg", timer = 40, loop = -1, op = {81}, dst = {
		{time = 0, x = 460, y = 100, w = 1000, h = 880, a = 255},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = -111, loop = 500, op = {80}, dst = {
		{time = 0, x = 660, y = 191, w = 600, h = 8, a = 0},
		{time = 250},
		{time = 500, a = 63}
	}})

	table.insert(skin.destination, {id = -111, timer = 40, loop = -1, op = {81}, dst = {
		{time = 0, x = 660, y = 191, w = 600, h = 8, a = 63},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = "loading-bar", loop = 500, op = {80}, dst = {
		{time = 0, x = 660, y = 191, w = 600, h = 8, a = 0},
		{time = 250},
		{time = 500, a = 255}
	}})

	table.insert(skin.destination, {id = "loading-bar", timer = 40, loop = -1, op = {81}, dst = {
		{time = 0, x = 660, y = 191, w = 600, h = 8, a = 255},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = -101, loop = 500, op = {80, 195}, blend = 2, filter = 1, stretch = 1, dst = {
		{time = 0, x = 662, y = 201, w = 596, h = 698, a = 0},
		{time = 250},
		{time = 500, a = 255}
	}})

	table.insert(skin.destination, {id = -101, timer = 40, loop = -1, op = {81, 195}, blend = 2, filter = 1, stretch = 1, dst = {
		{time = 0, x = 662, y = 201, w = 596, h = 698, a = 255},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = "loading-genre", loop = 500, op = {80, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 750, w = 840, h = 40, a = 0},
		{time = 250},
		{time = 500, a = 255}
	}})

	table.insert(skin.destination, {id = "loading-genre", timer = 40, loop = -1, op = {81, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 750, w = 840, h = 40, a = 255},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = "loading-artist", loop = 500, op = {80, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 320, w = 840, h = 40, a = 0},
		{time = 250},
		{time = 500, a = 255}
	}})

	table.insert(skin.destination, {id = "loading-artist", timer = 40, loop = -1, op = {81, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 320, w = 840, h = 40, a = 255},
		{time = 250, a = 0}
	}})

	table.insert(skin.destination, {id = "loading-title", loop = 500, op = {80, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 570, w = 840, h = 80, a = 0},
		{time = 250},
		{time = 500, a = 255}
	}})

	table.insert(skin.destination, {id = "loading-title", timer = 40, loop = -1, op = {81, 194}, filter = 1, dst = {
		{time = 0, x = 960, y = 570, w = 840, h = 80, a = 255},
		{time = 250, a = 0}
	}})
	
	if isPureModeOff() then
		table.insert(skin.destination, {id = "bg", loop = 250, timer = 2 , dst = {
			{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
			{time = 250, a = 255},
		}})
	else
		table.insert(skin.destination, {id = -110, loop = 250, timer = 2 , dst = {
			{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
			{time = 250, a = 255},
		}})
	end
	
	table.insert(skin.destination, {id = -110, loop = 500, timer = 2 , dst = {
		{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
		{time = 500, a = 255},
	}})

	if isPureModeOff() then
		table.insert(skin.destination, {id = "bg", loop = 250, timer = 3 , dst = {
			{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
			{time = 250, a = 255},
		}})
	else
		table.insert(skin.destination, {id = -110, loop = 250, timer = 3 , dst = {
			{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
			{time = 250, a = 255},
		}})
	end
	
	table.insert(skin.destination, {id = -110, loop = 500, timer = 3 , dst = {
		{time = 0, x = 0, y = 0, w = 1920, h = 1080, a = 0},
		{time = 500, a = 255},
	}})

	table.insert(skin.destination, {id = "fail", loop = -1, timer = 3 , dst = {
		{time = 450, x = 480, y = -507, w = 960, h = 2094, a = 0},
		{time = 500, x = 0, y = 284, w = 1920, h = 512, a = 255},
		{time = 599, a = 0},
		{time = 600, a = 225},
		{time = 699, a = 0},
		{time = 700, a = 225},
		{time = 2000, a = 225},
		{time = 2500, a = 0},
	}})

    return skin
end

return {
	header = header,
	main = main
}
