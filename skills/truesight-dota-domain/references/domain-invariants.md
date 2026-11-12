# TrueSight Dota Domain & Statistical Invariants Reference

## 1. 5v5 Probabilistic Engine Invariants (`truesight-core`)

1. **8 Individual Dota 2 Medals (`Bracket::ALL`)**:
   - `Herald (0)`, `Guardian (1)`, `Crusader (2)`, `Archon (3)`, `Legend (4)`, `Ancient (5)`, `Divine (6)`, `Immortal (7)`.
   - Execution sharpness factor: $\tau_b = [0.86, 0.90, 0.94, 0.97, 1.00, 1.03, 1.07, 1.12]$.
   - `Immortal` base winrate is extrapolated smoothly from ranked pub high-MMR trend ($W_7 + 0.35(W_7 - W_5)$) rather than tiny tournament `pro_pick` samples, guaranteeing $|P(\text{Divine}) - P(\text{Immortal})| < 3\%$.

2. **Pairwise Counter & Synergy Matrices (`127 x 127` = `16,002` pairs)**:
   - **Counter Antisymmetry**: $\Delta C_b(A, B) = \tau_b \Delta C_0(A, B) + 0.22 (d_{A,b} - d_{B,b}) = -\Delta C_b(B, A)$, where $d_{i,b}$ is zero-centered per bracket.
   - **Synergy Symmetry**: $\Delta S_b(A, B) = \tau_b \Delta S_0(A, B) + 0.075 (d_{A,b} + d_{B,b}) = \Delta S_b(B, A)$, double-centered to zero mean across the 127-hero catalog.
   - **Row Averages**: `ally_counter_avg[i]` and `ally_synergy_avg[i]` are pure arithmetic means of the displayed row cells.

3. **$\sqrt{N}$ Team Normalization & Combined Logit**:
   - $E_{\text{meta}} = \frac{1}{\sqrt{5}} \left(\sum_{i \in A}(W_{A_i}-0.5) - \sum_{j \in E}(W_{E_j}-0.5)\right)$
   - $E_{\text{counter}} = \frac{1}{\sqrt{|A|\cdot|E|}} \sum_{i,j} \Delta C_b(A_i, E_j)$
   - $E_{\text{synergy}} = E_{\text{syn,ally}} - E_{\text{syn,enemy}} = \frac{1}{\sqrt{\binom{|A|}{2}}}\sum_{i<k}\Delta S_b(A_i, A_k) - \frac{1}{\sqrt{\binom{|E|}{2}}}\sum_{j<l}\Delta S_b(E_j, E_l)$
   - $E_{\text{comp}} = C(A) - C(E)$ from `AttackType` (`Melee`/`Ranged`), `has_disable`, and `has_initiation`.
   - $\text{Logit}(P_{\text{win}}) = 3.2 \cdot \left(1.0 E_{\text{meta}} + 1.25 E_{\text{counter}} + 0.90 E_{\text{synergy}} + 0.85 E_{\text{comp}} + \frac{0.65}{\sqrt{5}} E_{\text{comfort}}\right)$.

## 2. Personal Comfort Engine v3 (`HeroProfile::comfort_delta`)

- Uses per-match exponential recency decay ($\tau = 75\text{ days}$, floor `0.0`) over `/api/players/{id}/matches?limit=500`:
  $$G_{\text{eff}}(i) = \sum_{k \in \text{matches}(i)} e^{-\Delta t_k / 75\text{d}}, \quad \text{comfort\_decay}_i = \text{clamp}\!\left(\frac{G_{\text{eff}}(i)}{N_{\text{total}}(i)}, 0.0, 1.0\right)$$
- Inactive threshold: if $G_{\text{eff}} < 0.15$, `comfort_delta() == 0.0`.
- Active comfort:
  $$\widehat{WR}_{\text{eff}} = \frac{W_{\text{eff}} + 8.0}{G_{\text{eff}} + 16.0}, \quad \Delta_{\text{comfort}} = \text{clamp}\!\left((\widehat{WR}_{\text{eff}} - 0.50) + \mathbb{I}(\widehat{WR}_{\text{eff}} \ge 0.50 \land G_{\text{eff}} \ge 0.50)\cdot 0.0065\ln(1+G_{\text{eff}}),\; -0.05,\; +0.05\right)$$

## 3. Local Cache & Synchronization (`LocalCacheManager`)

- Cache directory: `~/.local/share/truesight-dota/cache/`
- `opendota_hero_stats.json`: `META_TTL_SECS = 86_400` (**24 hours**).
- `player_heroes.json` + `player_matches.json`: `PLAYER_TTL_SECS = 3_600` (**1 hour**).
