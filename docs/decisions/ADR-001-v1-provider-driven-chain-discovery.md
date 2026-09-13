# ADR-001：v1 采用 provider 驱动的多链发现

- 状态：Accepted
- 日期：2026-09-12
- 范围：Rust runtime、provider 注册和 capabilities API

## 背景

Matcha Meta 的 DEX Aggregation 页面按 provider 列出支持网络，而不是提供一份适合本服务部署配置的统一 chain allowlist。不同 provider 支持的链集合不同；有些 provider 需要 access key，有些可以无 key 访问。v1 的目标是覆盖页面列出的 provider，并用最少的配置和最少的运行时分支完成同链竞争。

## 决策

1. 在代码中维护 Matcha 页面 provider-to-chain 能力矩阵。
2. 启动时创建固定的 provider 集合，按 provider 自己的 `supported_chains()` 结果生成 `HashMap<chain_id, Vec<string_id>>` 反向索引；结果与 chain catalog 求交集。
3. 不提供 chain enable/disable 配置，不提供 provider enable/disable 配置，不在配置中列 token。
4. 需要 access key 的 provider 在缺 key 时返回空 `supported_chains()`；不需要 key 的 provider 在没有 key 时仍返回自身支持链。配置 optional key 时传给对应 adapter，但不改变参与资格。
5. API 只对 token 地址和交易安全边界做验证；token 是否存在、是否有流动性、decimals 和价格由 provider/RPC/仿真决定。
6. RPC URL 是运行基础设施，不是能力 policy：使用 `RPC_URL_<chainId>` 环境变量提供。catalog 中没有 RPC 的链仍能被发现，但仿真会明确返回 unavailable/unsupported。

## 为什么不采用其他方案

### 用户配置 chain/provider allowlist

会重复表达 provider 已有能力，产生“配置允许但 provider 不支持”和“provider 支持但配置漏掉”的状态空间；也不符合 v1 的“默认全部参与”要求。后续需要运营降级时再新增显式 policy，当前不提前引入。

### 运行时拉取远端能力矩阵

启动结果会受远端可用性、版本漂移和缓存影响，服务无法稳定解释 capabilities。v1 先用来源可追踪的代码矩阵，升级由代码 review 和测试完成。

### token 白名单/Token 配置

会让聚合器变成 token registry，增加维护成本并拒绝 provider 已支持的新资产；同时无法保证 token 在特定链上真实可执行。v1 透传 token 地址，但仍保留输入格式、reserved address、route、value 和仿真校验。

### 通用 credential trait

provider 的认证形态不一致（API key、client ID、多字段签名、免 key），通用 credential abstraction 不会减少核心代码，反而会隐藏“缺 key 时支持链为 false”的边界。每个 adapter 直接持有自己的 optional credential。

## 后果

正面：配置极小；新增链只需更新 catalog/对应 adapter 矩阵；请求路径只查一次反向索引；缺必需 key 的 provider 不会触网；capabilities 可以准确反映本次进程实际可竞赛的 provider。

代价：能力矩阵需要随 Matcha 页面变化重新 review；链 catalog 目前是 EVM 名称/ID 的代码快照；provider API 适配仍必须逐家实现，不能因为能进入索引就宣称 live 可报价；跨链和 token metadata 不在本 ADR 范围。

## 验收

- 空配置生成 catalog 全集，但必需 key 缺失的 provider 不出现在任一 chain 的索引。
- 免 key provider 在空配置下出现在自身支持链的索引。
- 非 catalog chain 的请求返回 `INVALID_INPUT`。
- token 地址不在服务端 token 列表时，只要格式正确即可进入 provider quote 阶段。
- quote 阶段只调度对应 chain 的 provider；单个 provider 失败不影响其他结果。
