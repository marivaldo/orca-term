# Handoff: orca-term — TUI (direção "Quiet v2")

## Overview
orca-term é um "Orca em formato terminal": roda vários agentes de código CLI (claude-code, opencode) em paralelo, cada um no seu git worktree. Implementação alvo: **Rust + ratatui**, com os painéis do centro sendo PTYs reais (ex.: `portable-pty` + `vt100`/`tui-term` para renderizar).

## About the Design Files
`design/orca-term TUI.dc.html` é uma **referência de design em HTML**. Abra no navegador (precisa de `support.js` ao lado). Cada tela é um buffer de células gerado em JS (`class Buf` no `<script>`) que imita exatamente o `Buffer` do ratatui: 1 caractere × 1 linha, fg/bg por célula, bold/italic/underline. **Não** porte o HTML: recrie as telas em ratatui. As funções JS (`card`, `sidebar2`, `right2`, `tabs2`, `status2`, `nvim`, `claudeLines`, `mono`) dão coordenadas célula a célula e podem ser lidas como pseudocódigo.

A seção implementada é a **4 · Quiet v2** (4a–4g). As seções 1–3 são direções exploradas e descartadas. Use só os tokens da seção 3 que também estão em `theme.rs`.

## Fidelity
**Alta fidelidade.** Cores, posições em células, glifos e textos são finais. Nada fora da grade, sem Nerd Fonts, só Unicode padrão (box-drawing U+2500–257F, blocos/quadrantes U+2580–259F, ● ○ ◐ ◓ ◑ ◒ ✓ ✗ ? • › → ↑ ↓ −).

## Regra central de foco/seleção
- **Accent (#b9a2ff) só aparece onde as teclas vão.** Painel com foco: traço `▁` accent sobre o título/aba ativa (linha 0 do cartão) + título ou aba em accent + pílula em `FOCUS_SEL`. Lista com foco: o cursor também ganha `▎` accent na coluna 1 do item.
- **Seleção ≠ foco.** O worktree aberto fica num cartão `SELECTION` mesmo sem foco; com foco, o mesmo cartão vira `FOCUS_SEL` + `▎` accent + nome em accent.
- **Prefixo pendente (Ctrl-a):** a linha de status inteira vira `FOCUS_BG`, a pílula `^A` fica com fundo accent e as keycaps acendem (texto accent sobre `FOCUS_SEL`).

## Layout (180×48)
```
col 0      1..34        35  36..143 (108)            144 145..178 (34)   179
row 0      ▗▄▄▄▖ topo dos cartões (meia linha). Foco: '▁' accent em x+2.. sobre o título
rows 1..45 corpo: sidebar(PANEL) | centro(BG) | direita(PANEL)
row 46     ▝▀▀▀▘ base dos cartões (meia linha)
row 47     status bar (CANVAS, ou FOCUS_BG com prefixo)
```
Colunas 0, 35, 144 e 179 = `CANVAS` (calhas). **Cartão** (`card(x,y,w,h,bg,focus)`): linha y `▗`+`▄`×(w−2)+`▖` fg=bg do cartão sobre CANVAS; corpo y+1..y+h−2 preenchido com bg; linha y+h−1 `▝`+`▀`×(w−2)+`▘`. Se tiver foco: `▁`×N em accent a partir de x+2 (N = largura do título/aba; ex.: "1 agent" = 9, "2 nvim" = 8, "Projects" = 8, "changes" = 7).

**Split agent | nvim (4b):** o centro vira dois cartões: agent x36 w53, calha x89, nvim x90 w54. Cada um tem cabeçalho próprio e só o cartão com foco ganha o traço.
**Zoom (4d):** um cartão x1 w178. O cabeçalho mostra `split  agent | nvim` (painel atual em accent) + pílula `zoom` + keycap `^a z restore`.
**120×36 (4c):** sidebar x1 w28, centro x30 w89, painel direito oculto (resumo `4 files +55 −21 [^a f]` no cabeçalho do centro), status na linha 35, itens em modo compacto (2 linhas).

## Componentes
### Sidebar "Projects" (x1 w34)
- Linha 1: `Projects` bold (TEXT_2; ACCENT com foco). Sem ícones.
- Por projeto: avatar `▐a▌` (inicial do nome, RAISED/TEXT bold) em x+1, nome bold em x+5. Linha em branco.
- Item (3 linhas + 1 em branco), recuado:
  - L1: glifo de estado em x+4 (cor do estado, bold), nome bold em x+6, tag ` primary ` (TEXT_2 sobre RAISED) logo após o nome, `•` não visto (cor do estado) após o nome se não for o worktree aberto.
  - L2: branch em x+6 (TEXT_2); PR `#131` (DONE bold) alinhado à direita em x+w−3.
  - L3: check do PR (`✓/✗/◐`, se houver) + `claude · ` (TEXT_2 itálico) + última mensagem (TEXT_2, truncada com `…`) + idade alinhada à direita.
  - **O nome nunca trunca antes do resumo.**
- Selecionado: fundo em x+1..x+w−2 nas 3 linhas + `▄` na linha acima e `▀` na linha abaixo (mesma cor), formando um cartão de meia linha. Com foco: `FOCUS_SEL`, `▎` accent em x+1, nome em accent.
- Projeto vazio: `no worktrees yet` itálico TEXT_2.
- Rodapé (só com foco): keycaps `enter open  n new  / filter` (sem `/ filter` se w<32).
- Compacto (h<40): L1 + L2 = resumo + idade.

### Abas do centro (linha 1 do cartão)
`1 agent  2 nvim  3 shell`. A aba ativa é uma pílula `▐label▌`: SELECTION/TEXT sem foco, FOCUS_SEL/ACCENT com foco. As inativas ficam em TEXT_2. À direita, `app/fix-login · claude` (TEXT_2; o agente em itálico).

### Painel direito (x145 w34)
`changes  4` + `+55 −21` à direita. Arquivos: letra M(WORKING)/A(DONE)/D(FAILED) bold em x+2, nome base em TEXT, diretório em TEXT_2 (truncado à esquerda), `+N −N` à direita. Barra de diffstat `━` (DONE proporcional / FAILED) com largura w−4. Seções `pull request` e `branch` com rótulo bold + régua `─` em BORDER até a borda. Sem PR: `○ none yet` + keycap `^a P open a draft`. Com PR: `#131 título`, `draft · 1 failing · 1 running`, lista de checks (glifo + nome + duração à direita; falha em FAILED).

### Status bar
`▐orca▌` (ou `▐^A▌` accent com prefixo) · glifo do worktree + `app/fix-login` bold + painel (TEXT_2) · [`zoom`] · contagens `? 1 needs you  ✗ 1 failed  ◐ 1 running  ○ 2 quiet` (compacto: só glifo + número) · keycaps alinhadas à direita.
- Normal: `^a prefix  ^a h l focus  ^a j k worktree  ^a z zoom  ^a | split  ^a ? help`
- Prefixo: `h l focus  j k worktree  z zoom  | split  w picker  f files  a send ^A  esc cancel`
Keycap = ` k ` bold TEXT sobre RAISED (com prefixo: ACCENT sobre FOCUS_SEL).

### Falha do agente (4e)
Abaixo da saída do PTY, um cartão do próprio orca-term (SELECTION, cantos de meia linha): `✗  agent stopped · exit 1 after 41m · 12m ago`, uma linha dizendo que as mudanças estão salvas e as ações `r resume session` (keycap acesa = ação primária), `R restart clean`, `l show log`, `x dismiss`.

### Conteúdo dos terminais
É o próprio programa que desenha (PTY). Os mocks só mostram o resultado esperado com a paleta ANSI do tema: claude-code com pergunta de múltipla escolha (caixa arredondada WAITING) e Neovim (signcolumn `+`/`~`, números, cursorline PANEL, statusline com `NORMAL` em WORKING). **Mapeie as 16 cores ANSI do PTY para os tokens**: red=FAILED, green=DONE, yellow=WAITING, blue/cyan=WORKING, magenta=ACCENT, white=TEXT, bright-black=TEXT_2, bg padrão=BG.

## Interações (teclado; mouse opcional)
| Tecla | Ação |
|---|---|
| `Ctrl-a` | arma o prefixo (status em FOCUS_BG); timeout ~1.5 s ou `esc` |
| `^a h` / `^a l` | foco painel à esquerda/direita (sidebar ↔ centro ↔ direita; no split, agent ↔ nvim) |
| `^a j` / `^a k` | worktree seguinte/anterior (ordem da sidebar), abre direto |
| `^a z` | zoom do painel com foco / restaura (preserva split, tamanhos, foco; `h/l` em zoom troca de painel e mantém zoom) |
| `^a \|` | liga/desliga split agent \| nvim |
| `^a w` | seletor flutuante de worktrees |
| `^a f` | mostra/oculta painel de arquivos (em 120 col vira sobreposição) |
| `^a P` | abre PR rascunho |
| `^a a` | envia Ctrl-a literal ao PTY (bash/zsh usam Ctrl-a) |
| Sidebar com foco | `j/k` ou `↑/↓` movem o cursor, `enter` abre, `n` novo worktree, `/` filtro |
| Falha | `r` retoma, `R` reinicia limpo, `l` log, `x` dispensa |

Sem prefixo, as teclas vão para o painel com foco (PTY recebe tudo).

## Estado
- `projects: Vec<Project{name, base_branch, worktrees: Vec<Worktree>}>`
- `Worktree{ name, branch, primary, agent: Option<AgentKind>, state: State, last_message, updated_at, unseen, pr: Option<Pr{number, checks: Vec<Check>}>, changes: Vec<FileChange{kind, path, add, del}> }`
- `ui: { open_worktree, focus: Pane{Sidebar, Agent, Nvim, Shell, Right}, sidebar_cursor, layout: Single(Tab) | Split, zoom: bool, prefix_pending: Option<Instant>, picker_open, right_visible, spinner_frame }`
- `unseen` vira false ao abrir o worktree. Spinner avança a cada 120 ms só se houver algum `Working`.
- Contagens do status: needs you = Waiting; failed = Failed; running = Working; quiet = Done + Idle.

## Responsivo
≥ 150 col: 3 colunas (34 | flex | 34). 100–149: sidebar 28, painel direito oculto (resumo no cabeçalho). < 100: só o centro, sidebar via `^a w`. Altura < 40: itens compactos.

## NO_COLOR (4f, 4g)
Se `NO_COLOR` estiver definido: sem fg/bg. Foco = REVERSED+BOLD na pílula/cursor, título em BOLD+UNDERLINED. Seleção sem foco = réguas `─` acima/abaixo do item (substituem `▄▀`). Prefixo = status inteiro REVERSED. Secundário = DIM. Meias-linhas/quadrantes viram espaço. Ver `theme::mono`.

## Design Tokens
Ver `theme.rs` (hex + fallback ANSI-256 + papel). Contraste: todo token de texto ≥ 4.5:1 sobre CANVAS, BG, PANEL, RAISED, SELECTION, FOCUS_BG e FOCUS_SEL, em truecolor e em 256. Em 256 cores, SELECTION, FOCUS_SEL e FOCUS_BG colapsam para 236. O foco continua visível pelo accent (`▁`, `▎`, texto). Detecte truecolor via `COLORTERM=truecolor|24bit`.

Espaçamento em células: margem interna do cartão 2 col (conteúdo em x+2), recuo de item 4 (glifo) / 6 (texto), 1 linha entre itens, 1 linha extra entre projetos, calha de 1 col entre cartões.

## Assets
Nenhum. Só caracteres Unicode e cores.

## Files
- `design/orca-term TUI.dc.html`: todas as telas. Implementar 4a (principal), 4b (split + prefixo), 4d (zoom), 4c (120×36), 4e (falha), 4f/4g (NO_COLOR). A seção 3 tem a tabela de tokens com contraste calculado.
- `design/support.js`: runtime para abrir o HTML.
- `theme.rs`: tokens prontos para ratatui.
- `screenshots/`: PNGs 2x de cada tela: `4a-main-180x48`, `4b-split-prefix`, `4c-120x36`, `4d-zoom`, `4e-failed`, `4f-no-color`, `4g-no-color-120x36`.
