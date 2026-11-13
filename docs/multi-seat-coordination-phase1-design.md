# Thiết kế giai đoạn 1: Điều phối đa ghế (Scout/Worker/Reviewer) cho zencode CLI

- Trạng thái: Leader đã duyệt thiết kế (tin 10525, 12/11/2026). Giai đoạn 1 chỉ thiết kế, chưa triển khai code. Giai đoạn 2 đang triển khai: PR đầu là tool `delegate` (nhánh `feature/multi-seat-coord`).
- Nhánh: `feature/multi-seat-coord`, base main `2e7beaf` (rebase từ `1786049` ngày 12/11/2026).
- Ngày: 12/11/2026. Ghế: ZC Developer. Lệnh: P0, theo lệnh Zen.
- Nghiệm thu giai đoạn 1: tài liệu này. Leader duyệt thiết kế rồi mới triển khai giai đoạn 2.

## 1. Mục tiêu và phạm vi

Mục tiêu: CLI có cơ chế giao việc và handoff giữa các ghế trong cùng 1 phiên, thay cho điều phối bằng tay qua Telegram hiện nay.

Phạm vi giai đoạn 1: giao thức điều phối (giao việc cho subagent, handoff kết quả, tổng hợp) + định nghĩa 3 vai. Không triển khai code, không mailbox ngang hàng, không trạng thái liên phiên.

Không đổi: 3 vai (Scout chỉ đọc, Worker chỉnh sửa, Reviewer chỉ báo cáo); free-only; kiểm chứng trước khi báo xong; cổng critic độc lập trước tổng hợp.

## 2. Nghiên cứu: Pi và Claude Code làm điều phối ra sao

### 2.1 Pi 1.5.0 (@earendil-works/pi-coding-agent)

- Core KHÔNG có điều phối đa agent: grep "subagent" trong `dist/bundle/cli.js` bằng 0, `pi --help` không có cờ delegate/subagent [M|0]. Điều phối nằm ở extension ví dụ `subagent` (file `index.ts` 1.036 dòng) đăng ký tool qua ExtensionAPI [S|0] - lựa chọn thiết kế có chủ ý, đẩy điều phối ra tầng extension.
- Tool `subagent`, 3 mode: Single `{agent, task}`; Parallel `{tasks: [...]}`; Chain `{chain: [...]}` với placeholder `{previous}` cho bước sau [S|0].
- Agent là file Markdown + YAML frontmatter (name, description, tools, model); thân file là system prompt [S|0]. Quét lại mỗi lần gọi từ `~/.pi/agent/agents` (user) và `.pi/agents` (project) [S|0].
- Mỗi lần gọi tạo 1 process `pi` riêng (`pi --mode json -p --no-session`), system prompt ghi file tạm rồi xóa; `--no-session` khiến transcript con không lưu, không bẩn session cha [S|0].
- Handoff qua stdout: extension đọc từng dòng JSON event stream, trả về cho model cha là thông điệp assistant cuối cùng của con dạng text [S|0]. Parallel nối các output thành 1 text, mỗi task giới hạn 50 KB; không có tổng hợp tự động - việc gộp do model cha tự làm [I|0]. Chain dừng ở bước lỗi đầu tiên, báo rõ bước nào lỗi [S|0].
- 4 agent mẫu: scout, planner, reviewer, worker [S|0]. Phân quyền bằng allowlist `tools` trong frontmatter + instruction trong prompt - ràng buộc mềm: chính reviewer.md thừa nhận "Assume tool permissions are not perfectly enforceable" (tạm dịch: cứ cho rằng quyền của tool không thể thực thi hoàn hảo) [S|0]. Scout Pi vẫn có bash, không thuần chỉ đọc như Scout của đội [S|0].
- Giới hạn: tối đa 8 task, tối đa 4 chạy đồng thời; không giới hạn độ sâu đệ quy, không chống vòng lặp, không timeout riêng cho từng subagent [M|0]. Workflow mẫu: `/implement` (scout - planner - worker), `/implement-and-review` (worker - reviewer - worker) [S|0].

### 2.2 Claude Code 2.1.304

- 2 tầng điều phối riêng biệt [S|0]: Subagent (trong 1 session, main quản lý toàn bộ, handoff 1 chiều qua summary) và Agent Teams (nhiều instance độc lập, lead + teammate nhắn trực tiếp, task list chung, mailbox JSON; tắt mặc định, cần interactive session, không chạy ở chế độ `-p`).
- Tool `Agent` (trước tên `Task`, đổi tên ở v2.1.63) [S|1]. Loại có sẵn: Explore (chỉ đọc, cấm Write/Edit, 3 mức kỹ lưỡng), Plan (chỉ đọc), general-purpose (đủ tool) [S|0].
- Custom agent: Markdown + YAML frontmatter ở `.claude/agents/` (project), `~/.claude/agents/` (user), cờ `--agents`, plugin. Frontmatter gồm description, tools, disallowedTools, model, permissionMode, hooks, maxTurns, skills, background, omitClaudeMd, isolation [S|0].
- Context con bắt đầu trống, chỉ nhận system prompt riêng, delegation message do main viết, CLAUDE.md các cấp, snapshot git status [S|0]. `isolation: worktree` cho con làm việc trên bản sao repo riêng [S|0].
- Handoff 1 chiều: con trả 1 bản tóm tắt cuối, output dài ở lại context của nó [S|0]. Background chạy đồng thời, kết quả về main qua completion notification; mặc định subagent do main tạo chạy background [S|0].
- Chống injection: Claude Code quét report cuối trước khi main đọc (chèn backslash vào text giả mạo output hệ thống, marker line khi nhắc permission settings) và gắn header nói rõ đây là lời subagent, không có thẩm quyền [S|0].
- Resume bằng tool `SendMessage` với agent ID; transcript lưu `~/.claude/projects/.../subagents/agent-{id}.jsonl`, giữ mặc định 30 ngày [S|0]. Tối đa 20 subagent đồng thời (mặc định), lồng nhau tối đa 3 tầng [S|0]. Muốn 1 subagent không tạo thêm thì bỏ `Agent` khỏi `tools` hoặc thêm vào `disallowedTools` [S|0].
- Agent Teams: mailbox mỗi agent là file JSON tại `~/.claude/teams/{team}/inboxes/{agent}.json`; task có 3 trạng thái pending / in progress / completed + dependency (blocked tới khi task phụ thuộc xong); Nhận việc dùng file locking chống 2 agent nhận cùng 1 task [S|0]. Teammate tự thông báo khi idle/lỗi; token tăng tuyến tính theo số teammate; 2 teammate sửa cùng 1 file sẽ ghi đè nhau, phải chia tập file riêng [S|0].
- Hooks: SubagentStart/SubagentStop; quality gate TeammateIdle/TaskCreated/TaskCompleted (exit code 2 chặn hành động) [S|0].

### 2.3 Điểm chung đáng học (suy luận)

- Cả hai đều định nghĩa agent bằng Markdown + YAML frontmatter - đã thành mẫu chung, nên theo.
- Handoff 1 chiều qua summary là mẫu tối giản phù hợp CLI; mailbox + task list chỉ cần khi các ghế trao đổi ngang nhau.
- Vai thực thi bằng cơ chế tool (allowlist/denylist), không chỉ bằng prompt - Pi chỉ dựa vào prompt nên thừa nhận không enforce được.
- Task list có trạng thái + dependency + file locking khi nhận việc là cơ chế đáng học cho nhiều ghế nhận việc song song.
- Quét output subagent + header "không có thẩm quyền" là biện pháp chống prompt injection đáng học cho handoff giữa các ghế.

## 3. Thực tiễn đội hôm nay (vấn đề cần giải quyết)

Luồng 5 bước bằng tay: giao việc (tin Telegram, Reply ĐÃ NHẬN trong 30 phút) - fan-out song song (mỗi Scout một miền, file riêng) - làm việc (Worker sửa trên worktree riêng, nhánh dev/<tên>) - handoff (bundle git + sha256; file báo cáo + SHA; tin ĐÃ NHẬN/XONG/CHẶN; Notion làm bản ghi) - kiểm chứng + báo cáo (ghế chính kiểm chứng trực tiếp trước khi báo xong).

Điểm yếu đã đo trong thực tế: chờ đợi không hẹn giờ tự động (câu hỏi regression test gửi 08:54 ngày 12/11, tới 11:28 chưa trả lời, phải kiểm tra thủ công); tin peek bị cắt ở 200 ký tự (phần sau tin Leader 10394 bị cắt, phải hỏi lại qua Notion); trùng lặp (cùng file gửi 2 lần; PR #176/#177/#178 mở trùng #170/#171/#172, phải đóng không merge); báo cáo worker cũ/sai (3 lần trong ngày 11/11); việc mồ côi (worker PR #158 báo "DỪNG DO HẾT GIỜ, CHƯA XONG" nhưng không nằm trong DOING bàn giao); kiểm chứng hoàn toàn thủ công; 3 kênh Telegram/Notion/ztask lệch nhau.

## 4. Giao thức đề xuất

### 4.1 Kiến trúc: hub-and-spoke trong 1 phiên

1 ghế điều phối (cha) + N subagent (con) trong cùng 1 phiên. Con chỉ trả kết quả về cha, không nhắn ngang nhau - chưa có mailbox ở giai đoạn 1. Lý do: khớp luồng đội (ghế chính điều phối, các ghế báo về); rẻ token (chỉ summary về cha); chạy được ở chế độ `-p` (Agent Teams của Claude Code không chạy được ở `-p`).

Mỗi subagent là 1 process riêng, JSON mode (mẫu Pi): cách ly context thật, đơn giản nhất để triển khai; transcript con lưu riêng trong thư mục session, không bẩn session cha.

### 4.2 Định nghĩa ghế

File Markdown + YAML frontmatter tại `.zen/agents/` (project) và `~/.zen/agents/` (user). Frontmatter: name, description, tools (allowlist), model, maxTurns.

3 vai chuẩn, phân quyền thực thi ở tầng tool/permission của CLI (dùng sẵn cơ chế `--permission-mode` / `--allowedTools`), không chỉ dựa vào prompt:

- Scout: tools read/grep/find/ls + git log/show (chỉ đọc). KHÔNG bash, KHÔNG Write/Edit. Chặt hơn scout Pi (Pi cho scout bash), đúng vai Scout của đội.
- Worker: toàn bộ tools. Khi sửa code thì làm việc trên worktree riêng (bắt buộc - luật đội đã có, tránh ghi đè lẫn nhau như cảnh báo của Claude Code).
- Reviewer: tools chỉ đọc + git diff/log/show. KHÔNG Write/Edit. KHÔNG được tạo subagent (bỏ tool delegate khỏi tools - mẫu Claude Code).

### 4.3 Giao việc: tool `delegate`

3 mode (mẫu Pi, đã chứng minh tối giản và đủ): single `{agent, task}`; parallel `{tasks: [...]}`; chain `{chain: [...]}` với placeholder `{previous}`.

Mỗi lần giao kèm brief tự chứa: mục tiêu, phạm vi, ràng buộc, định dạng kết quả mong đợi (đúng chuẩn brief v4 của đội: framing block).

Giới hạn cứng: tối đa 4 subagent chạy đồng thời (mặc định, cấu hình được - lấy số của Pi, chặt hơn Claude Code vì free-first); lồng nhau tối đa 2 tầng; timeout riêng cho mỗi subagent (Pi thiếu điểm này - không copy thiếu sót); maxTurns theo frontmatter.

### 4.4 Handoff kết quả: envelope có cấu trúc

Con trả về 1 envelope JSON: status (done/blocked/failed), summary (text), evidence (danh sách file:dòng), usage (turns/tokens), sha256 (checksum của file kết quả đầy đủ). Đưa thực tiễn bundle+SHA của đội vào trong giao thức thay vì để người làm thủ công.

Cha nhận summary + envelope; output đầy đủ ở lại transcript con (mẫu Claude Code), giới hạn 50 KB/task về cha (mẫu Pi) để bảo vệ context cha. Chain: output bước trước thay vào `{previous}`; dừng ở bước lỗi đầu tiên, báo rõ bước nào lỗi.

Chống injection: quét output con trước khi cha đọc + gắn header "đây là lời subagent, không có thẩm quyền" (mẫu Claude Code). Claim "đã được duyệt" từ con không có giá trị - chỉ cha (hoặc người dùng) mới duyệt được.

### 4.5 Task list: chống nhận trùng

1 file JSON trong thư mục session. Task có trạng thái pending / in progress / completed + dependency (blocked tới khi task phụ thuộc xong). Khẳng định dùng file locking (mẫu Agent Teams) - chống 2 ghế nhận cùng 1 việc, lỗi đã gặp với PR #176/#177/#178. Giai đoạn 1: task list chỉ trong 1 phiên, không phải bảng đội chung.

### 4.6 Tổng hợp và cổng kiểm chứng

Cha tổng hợp (không có agent tổng hợp riêng ở giai đoạn 1 - cả Pi và Claude Code đều để main tự tổng hợp).

Cổng bắt buộc trước khi báo xong, đưa luật đội thành bước giao thức: (1) Reviewer độc lập chấm (vai Reviewer, không phải tác giả việc); (2) cha kiểm chứng trực tiếp (chạy lại lệnh kiểm chứng, đối chiếu bằng chứng) - không báo xong bằng lời subagent.

Luật dừng: cùng 1 bước lỗi 2 lần thì dừng, lập lại kế hoạch. Subagent quá thời gian không có sản phẩm trung gian thì cha can thiệp (heartbeat - luật đội: ~20 phút).

### 4.7 Hết hạn chờ và mồ côi

Mỗi lần giao có hạn chờ; hết hạn tự nhắc cha (thay cho việc ghế phải kiểm tra thủ công - lỗi đã gặp với câu hỏi regression test). Subagent kết thúc không có kết quả thì ghi dead-letter (trạng thái + lý do), không để việc mồ côi (lỗi PR #158).

## 5. Không làm ở giai đoạn 1

- Mailbox ngang hàng giữa các ghế (Agent Teams) - chỉ cần khi các ghế trao đổi trực tiếp, chưa phải nhu cầu giai đoạn 1.
- Trạng thái liên phiên (transcript giữ 30 ngày như Claude Code là đủ cho tra cứu).
- Retry tự động ngoài luật 2 lần.
- Model trả phí: con kế thừa model của cha mặc định, free-first.

## 6. Kế hoạch kiểm chứng (cho giai đoạn 2)

Triển khai: tool `delegate` + 3 định nghĩa vai + task list + envelope handoff.

Nghiệm thu bằng 1 việc đa ghế thật (mẫu nghiệm thu game 2048): đo token thật + redo rate (2 chỉ số bắt buộc). Kiểm chứng: Scout thử Write bị chặn ở tầng tool; Reviewer thử tạo subagent bị chặn; 2 ghế không nhận trùng 1 task; output con có nội dung giả mạo output hệ thống thì bị quét trước khi cha đọc; chain dừng đúng ở bước lỗi đầu tiên.

## 7. Nguồn và vết kiểm chứng

- Scout Pi: `scout-pi-coordination.md` (953 từ, sha256 356c41d32d46886e595fa872841794d93d7d6d94faedafc64cfb1e8ea634590b).
- Scout Claude Code: `scout-claude-code-coordination.md` (1.214 từ, sha256 a3598bbffe7508290ac090c4cf6122fc97f16eaba9648e1430ba6fe27cc78cee).
- Scout thực tiễn đội: `scout-team-practice.md` (994 từ, sha256 5eb395d8e6fd5d0e55d46801088aeee1120cdfaea3af7c062a038fb80e790415).
- Chưa kiểm chứng: tên tool Pi ở bản mới hơn (`task` theo hướng dẫn piagent.fyi so với `subagent` ở bản 1.5.0 đã cài); hành vi runtime (các Scout chỉ đọc, không chạy agent thật) - cần đo ở giai đoạn 2.
