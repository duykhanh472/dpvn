import unicodedata

# Common Vietnamese onsets, nuclei, codas, tones
# Generate high-frequency syllables + base vocabulary
onsets = ["", "b", "c", "ch", "d", "đ", "g", "gh", "gi", "h", "k", "kh", "l", "m", "n", "ng", "ngh", "nh", "p", "ph", "qu", "r", "s", "t", "th", "tr", "v", "x"]

# Standard common words and proper nouns
common_roots = [
    # Pronouns & People
    "tôi", "ta", "tao", "tớ", "mình", "chúng", "nó", "họ", "hắn", "nàng", "chàng", "người", "kẻ", "ai",
    "ông", "bà", "bác", "chú", "cô", "dì", "thím", "cậu", "mợ", "anh", "chị", "em", "con", "cháu", "chắt", "cụ",
    "cha", "mẹ", "bố", "thầy", "u", "tía", "má", "vợ", "chồng", "bạn", "bè", "trò", "vua", "quan",
    # Body
    "đầu", "mặt", "mắt", "mũi", "tai", "miệng", "môi", "răng", "lưỡi", "cằm", "cổ", "vai", "ngực", "lưng", "bụng",
    "tay", "chân", "ngón", "móng", "da", "thịt", "xương", "máu", "tim", "gan", "phổi", "ruột", "tóc", "râu", "lông",
    # Nature & Time
    "trời", "đất", "nước", "lửa", "gió", "mây", "mưa", "nắng", "sấm", "sét", "bão", "sương", "khói", "bụi", "tuyết",
    "sông", "suối", "hồ", "ao", "biển", "đại", "dương", "núi", "đồi", "rừng", "hang", "động", "bãi", "bờ", "cát", "đá",
    "ngày", "đêm", "sáng", "trưa", "chiều", "tối", "khuya", "hôm", "mai", "qua", "kịp", "giờ", "phút", "giây", "khắc",
    "tuần", "tháng", "năm", "mùa", "xuân", "hạ", "thu", "đông", "thời", "gian", "thuở", "xưa", "nay",
    # Animals & Plants
    "chó", "mèo", "gà", "vịt", "ngan", "ngỗng", "lợn", "heo", "bò", "trâu", "ngựa", "dê", "cừu", "hổ", "báo", "sư",
    "tử", "voi", "gấu", "hươu", "nai", "khỉ", "vượn", "chuột", "thỏ", "rắn", "rùa", "cá", "tôm", "cua", "ốc", "ếch",
    "chim", "quạ", "sáo", "khướu", "yến", "oanh", "cò", "vạc", "ong", "bướm", "kiến", "muỗi", "ruồi", "gián", "giun",
    "cây", "cỏ", "hoa", "lá", "cành", "gốc", "rễ", "thân", "vỏ", "búp", "nụ", "quả", "trái", "hạt", "hột", "mầm",
    "lúa", "gạo", "nếp", "tẻ", "ngô", "bắp", "khoai", "sắn", "đậu", "đỗ", "vừng", "mè", "rau", "muống", "cải", "bầu",
    # House & Objects
    "nhà", "cửa", "ngõ", "sân", "vườn", "tường", "vách", "mái", "cột", "kèo", "bậc", "thang", "buồng", "phòng", "bếp",
    "bàn", "ghế", "giường", "chiếu", "chăn", "màn", "gối", "tủ", "hòm", "rương", "nồi", "niêu", "xoong", "chảo", "bát",
    "đĩa", "chén", "tách", "đũa", "thìa", "muỗng", "dao", "kéo", "thớt", "chày", "cối", "chổi", "gầu", "thau", "chậu",
    "áo", "quần", "khăn", "mũ", "nón", "giày", "dép", "guốc", "váy", "yếm", "thắt", "lưng", "túi", "ví", "nơ", "kim",
    "chỉ", "vải", "lụa", "gấm", "vóc", "sách", "vở", "bút", "mực", "giấy", "thước", "bảng", "phấn", "tranh", "ảnh",
    # Colors & Qualities
    "đỏ", "xanh", "vàng", "trắng", "đen", "tím", "hồng", "nâu", "xám", "lam", "chàm", "tươi", "thắm", "nhạt", "đậm",
    "lớn", "bé", "to", "nhỏ", "dài", "ngắn", "cao", "thấp", "rộng", "hẹp", "sâu", "nông", "dày", "mỏng", "nặng", "nhẹ",
    "nhanh", "chậm", "mau", "lẹ", "sớm", "muộn", "trễ", "gần", "xa", "chặt", "lỏng", "rắn", "mềm", "cứng", "dai", "giòn",
    "nóng", "lạnh", "ấm", "mát", "ngon", "ngọt", "đắng", "cay", "chua", "chát", "mặn", "bùi", "thơm", "thối", "tanh",
    "đẹp", "xấu", "xinh", "duyên", "tốt", "hay", "dở", "khéo", "vụng", "đúng", "sai", "thật", "giả", "thẳng", "cong",
    "sạch", "bẩn", "dơ", "rõ", "mờ", "sáng", "tối", "yên", "lặng", "ồn", "động", "vui", "buồn", "sướng", "khổ", "lo",
    # Verbs & Actions
    "ăn", "uống", "hút", "hít", "thở", "ngủ", "thức", "dậy", "nằm", "ngồi", "đứng", "đi", "chạy", "nhảy", "bò", "trườn",
    "nói", "bảo", "kể", "hỏi", "đáp", "thưa", "gọi", "kêu", "la", "hét", "than", "thở", "hát", "ca", "ngâm", "đọc",
    "nghe", "thấy", "nhìn", "ngắm", "trông", "xem", "ngó", "dòm", "liếc", "thăm", "viếng", "chào",
    "nghĩ", "suy", "tưởng", "nhớ", "quên", "biết", "hiểu", "rõ", "thương", "yêu", "mến", "quý", "ghét", "hờn", "giận",
    "sợ", "hãi", "kinh", "hoảng", "mừng", "vui", "buồn", "khóc", "cười", "tiếc", "hận", "thù", "oán", "ghen", "tuông",
    "làm", "tạo", "chế", "xây", "dựng", "sửa", "chữa", "phá", "hủy", "mua", "bán", "trao", "đổi", "vay", "mượn", "trả",
    "cho", "tặng", "biếu", "ban", "lấy", "cầm", "nắm", "giữ", "bỏ", "buông", "ném", "vứt", "quăng", "tìm", "kiếm", "gặp",
    "thua", "thắng", "được", "mất", "hơn", "kém", "thừa", "thiếu", "đủ", "đầy", "vơi", "rỗng", "chứa", "đựng", "mang",
    # Grammar & Linking
    "là", "có", "không", "chẳng", "chưa", "chớ", "đừng", "đã", "đang", "sẽ", "vừa", "mới", "sắp", "còn", "vẫn", "cứ",
    "phải", "nên", "cần", "được", "bị", "rất", "quá", "lắm", "hơi", "khá", "thật", "quả", "hết", "cực", "vô", "cùng",
    "và", "với", "cùng", "hay", "hoặc", "nhưng", "mà", "song", "tuy", "dù", "mặc", "dầu", "vì", "bởi", "do", "tại",
    "nên", "cho", "để", "của", "về", "trong", "ngoài", "trên", "dưới", "trước", "sau", "giữa", "bên", "cạnh", "từ",
    "đến", "tới", "qua", "lại", "ra", "vào", "lên", "xuống", "sang", "ngang", "dọc", "khắp", "suốt", "chung", "riêng",
    # Culture, History, Proper Nouns
    "việt", "nam", "hà", "nội", "huế", "sài", "gòn", "đà", "nẵng", "hải", "phòng", "thăng", "long", "đông", "đô",
    "bắc", "trung", "tây", "kinh", "kỳ", "sông", "hồng", "cửu", "long", "bạch", "đằng", "lam", "sơn", "hoa", "lư",
    "nguyễn", "du", "trần", "hưng", "đạo", "lê", "lợi", "quang", "trung", "ngô", "quyền", "đinh", "bộ", "lĩnh",
    "lý", "thường", "kiệt", "thái", "tổ", "tông", "vương", "hoàng", "đế", "hậu", "phi", "chúa", "tướng", "quân", "sĩ",
    "kiều", "kim", "trọng", "thúy", "vân", "từ", "hải", "hoạn", "thư", "thúc", "sinh", "giác", "duyên", "đạm", "tiên",
    "chùa", "miếu", "đình", "đền", "tháp", "lăng", "tẩm", "phủ", "huyện", "tỉnh", "thành", "xã", "thôn", "làng", "ấp"
]

vowel_nuclei = [
    ("a", ["a", "à", "á", "ả", "ã", "ạ"]),
    ("an", ["an", "àn", "án", "ản", "ãn", "ạn"]),
    ("ang", ["ang", "àng", "áng", "ảng", "ãng", "ạng"]),
    ("anh", ["anh", "ành", "ánh", "ảnh", "ãnh", "ạnh"]),
    ("am", ["am", "àm", "ám", "ảm", "ãm", "ạm"]),
    ("ap", ["áp", "ạp"]),
    ("at", ["át", "ạt"]),
    ("ac", ["ác", "ạc"]),
    ("ach", ["ách", "ạch"]),
    ("ai", ["ai", "ài", "ái", "ải", "ãi", "ại"]),
    ("ao", ["ao", "ào", "áo", "ảo", "ão", "ạo"]),
    ("ay", ["ay", "ày", "áy", "ảy", "ãy", "ạy"]),
    ("au", ["au", "àu", "áu", "ảu", "ãu", "ạu"]),
    ("e", ["e", "è", "é", "ẻ", "ẽ", "ẹ"]),
    ("en", ["en", "èn", "én", "ẻn", "ẽn", "ẹn"]),
    ("eng", ["eng", "èng", "éng", "ẻng", "ẽng", "ẹng"]),
    ("em", ["em", "èm", "ém", "ẻm", "ẽm", "ẹm"]),
    ("ep", ["ép", "ẹp"]),
    ("et", ["ét", "ẹt"]),
    ("ec", ["éc", "ẹc"]),
    ("eo", ["eo", "èo", "éo", "ẻo", "ẽo", "ẹo"]),
    ("ê", ["ê", "ề", "ế", "ể", "ễ", "ệ"]),
    ("ên", ["ên", "ền", "ến", "ển", "ễn", "ện"]),
    ("ênh", ["ênh", "ềnh", "ếnh", "ểnh", "ễnh", "ệnh"]),
    ("êm", ["êm", "ềm", "ếm", "ểm", "ễm", "ệm"]),
    ("êp", ["ếp", "ệp"]),
    ("êt", ["ết", "ệt"]),
    ("êch", ["ếch", "ệch"]),
    ("êu", ["êu", "ều", "ếu", "ểu", "ễu", "ệu"]),
    ("i", ["i", "ì", "í", "ỉ", "ĩ", "ị"]),
    ("in", ["in", "ìn", "ín", "ỉn", "ĩn", "ịn"]),
    ("inh", ["inh", "ình", "ính", "ỉnh", "ĩnh", "ịnh"]),
    ("im", ["im", "ìm", "ím", "ỉm", "ĩm", "ịm"]),
    ("ip", ["íp", "ịp"]),
    ("it", ["ít", "ịt"]),
    ("ich", ["ích", "ịch"]),
    ("iu", ["iu", "ìu", "íu", "ỉu", "ĩu", "ịu"]),
    ("ia", ["ia", "ìa", "ía", "ỉa", "ĩa", "ịa"]),
    ("iên", ["iên", "iền", "iến", "iển", "iễn", "iện"]),
    ("iêng", ["iêng", "iềng", "iếng", "iểng", "iễng", "iệng"]),
    ("iêm", ["iêm", "iềm", "iếm", "iểm", "iễm", "iệm"]),
    ("iêp", ["iếp", "iệp"]),
    ("iêt", ["iết", "iệt"]),
    ("iêc", ["iếc", "iệc"]),
    ("iêu", ["iêu", "iều", "iếu", "iểu", "iễu", "iệu"]),
    ("o", ["o", "ò", "ó", "ỏ", "õ", "ọ"]),
    ("on", ["on", "òn", "ón", "ỏn", "õn", "ọn"]),
    ("ong", ["ong", "òng", "óng", "ỏng", "õng", "ọng"]),
    ("om", ["om", "òm", "óm", "ỏm", "õm", "ọm"]),
    ("op", ["óp", "ọp"]),
    ("ot", ["ót", "ọt"]),
    ("oc", ["óc", "ọc"]),
    ("oi", ["oi", "òi", "ói", "ỏi", "õi", "ọi"]),
    ("oa", ["oa", "oà", "oá", "oả", "oã", "oạ", "òa", "óa", "ỏa", "õa", "ọa"]),
    ("oan", ["oan", "oàn", "oán", "oản", "oãn", "oạn"]),
    ("oang", ["oang", "oàng", "oáng", "oảng", "oãng", "oạng"]),
    ("ô", ["ô", "ồ", "ố", "ổ", "ỗ", "ộ"]),
    ("ôn", ["ôn", "ồn", "ốn", "ổn", "ỗn", "ộn"]),
    ("ông", ["ông", "ồng", "ống", "ổng", "ỗng", "ộng"]),
    ("ôm", ["ôm", "ồm", "ốm", "ổm", "ỗm", "ộm"]),
    ("ôp", ["ốp", "ộp"]),
    ("ôt", ["ốt", "ột"]),
    ("ôc", ["ốc", "ộc"]),
    ("ôi", ["ôi", "ồi", "ối", "ổi", "ỗi", "ội"]),
    ("ơ", ["ơ", "ờ", "ớ", "ở", "ỡ", "ợ"]),
    ("ơn", ["ơn", "ờn", "ớn", "ởn", "ỡn", "ợn"]),
    ("ơm", ["ơm", "ờm", "ớm", "ởm", "ỡm", "ợm"]),
    ("ơp", ["ớp", "ợp"]),
    ("ơt", ["ớt", "ợt"]),
    ("ơi", ["ơi", "ời", "ới", "ởi", "ỡi", "ợi"]),
    ("u", ["u", "ù", "ú", "ủ", "ũ", "ụ"]),
    ("un", ["un", "ùn", "ún", "ủn", "ũn", "ụn"]),
    ("ung", ["ung", "ùng", "úng", "ủng", "ũng", "ụng"]),
    ("um", ["um", "ùm", "úm", "ủm", "ũm", "ụm"]),
    ("up", ["úp", "ụp"]),
    ("ut", ["út", "ụt"]),
    ("uc", ["úc", "ục"]),
    ("ui", ["ui", "ùi", "úi", "ủi", "ũi", "ụi"]),
    ("ua", ["ua", "ùa", "úa", "ủa", "ũa", "ụa"]),
    ("uôn", ["uôn", "uồn", "uốn", "uển", "uễn", "uộn"]),
    ("uông", ["uông", "uồng", "uống", "uổng", "uỗng", "uộng"]),
    ("uôi", ["uôi", "uồi", "uối", "uổi", "uỗi", "uội"]),
    ("uôt", ["uốt", "uột"]),
    ("uôc", ["uốc", "uộc"]),
    ("ư", ["ư", "ừ", "ứ", "ử", "ữ", "ự"]),
    ("ưng", ["ưng", "ừng", "ứng", "ửng", "ững", "ựng"]),
    ("ưa", ["ưa", "ừa", "ứa", "ửa", "ữa", "ựa"]),
    ("ươn", ["ươn", "ườn", "ướn", "ưởn", "ưỡn", "ượn"]),
    ("ương", ["ương", "ường", "ướng", "ưởng", "ưỡng", "ượng"]),
    ("ươm", ["ươm", "ườm", "ướm", "ưởm", "ưỡm", "ượm"]),
    ("ươp", ["ướp", "ượp"]),
    ("ươt", ["ướt", "ượt"]),
    ("ươc", ["ước", "ược"]),
    ("ươi", ["ươi", "ười", "ưới", "ưởi", "ưỡi", "ượi"]),
    ("ươu", ["ươu", "ườu", "ứu", "ửu", "ữu", "ựu"]),
    ("y", ["y", "ỳ", "ý", "ỷ", "ỹ", "ỵ"]),
    ("yên", ["yên", "yền", "yến", "yển", "yễn", "yện"]),
    ("yêu", ["yêu", "yều", "yếu", "yểu", "yễu", "yệu"])
]

words_set = set()

for w in common_roots:
    words_set.add(unicodedata.normalize("NFC", w.strip().lower()))

# Generate common onset + vowel pairs
selected_onsets = ["", "b", "c", "ch", "d", "đ", "g", "h", "k", "kh", "l", "m", "n", "ng", "nh", "p", "ph", "qu", "r", "s", "t", "th", "tr", "v", "x"]
for o in selected_onsets:
    for base, variants in vowel_nuclei:
        # Check spelling rule: k before i/e/ê/y; c before a/o/u
        if o == "k" and not (base.startswith("i") or base.startswith("e") or base.startswith("ê") or base.startswith("y")):
            continue
        if o == "c" and (base.startswith("i") or base.startswith("e") or base.startswith("ê") or base.startswith("y")):
            continue
        if o == "g" and (base.startswith("i") or base.startswith("e") or base.startswith("ê")):
            continue
        if o == "ng" and (base.startswith("i") or base.startswith("e") or base.startswith("ê")):
            continue
        if o == "gh" and not (base.startswith("i") or base.startswith("e") or base.startswith("ê")):
            continue
        if o == "ngh" and not (base.startswith("i") or base.startswith("e") or base.startswith("ê")):
            continue
            
        for v in variants:
            w = unicodedata.normalize("NFC", f"{o}{v}")
            words_set.add(w)

sorted_words = sorted(list(words_set))
print(f"Total unique starter words: {len(sorted_words)}")

# Write words.vi.txt
with open("crates/dp-db/words.vi.txt", "w", encoding="utf-8") as f:
    for w in sorted_words:
        f.write(f"{w}\n")

# Write migration 0002_seed_words.sql
with open("crates/dp-db/migrations/0002_seed_words.sql", "w", encoding="utf-8") as f:
    f.write("-- Migration 0002: Seed initial site words list (§7.5)\n")
    f.write("-- Autogenerated from words.vi.txt in NFC format\n\n")
    
    # Chunk into batch inserts of 500
    chunk_size = 500
    for i in range(0, len(sorted_words), chunk_size):
        chunk = sorted_words[i:i+chunk_size]
        f.write("INSERT IGNORE INTO site_words (kind, word, added_by, added_at) VALUES\n")
        lines = []
        for w in chunk:
            escaped = w.replace("'", "''")
            lines.append(f"  ('good', '{escaped}', NULL, UTC_TIMESTAMP(6))")
        f.write(",\n".join(lines) + ";\n\n")
