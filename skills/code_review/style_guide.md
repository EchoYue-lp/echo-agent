# 代码风格规范

## 命名约定

| 元素 | 风格 | 示例 |
|------|------|------|
| 变量/函数 | snake_case | `user_count`, `get_user()` |
| 类型/Trait | PascalCase | `UserService`, `Serialize` |
| 常量 | SCREAMING_SNAKE_CASE | `MAX_RETRY_COUNT` |
| 模块 | snake_case | `user_service` |

## 函数规范

- 单一函数不超过 50 行
- 参数不超过 5 个（过多时考虑参数对象）
- 嵌套层级不超过 3 层

## 注释规范

```rust
// ✅ 好的注释：解释"为什么"
// 使用指数退避避免雪崩效应
let delay = base_delay * 2u64.pow(retry_count);

// ❌ 坏的注释：仅重复代码内容
// 将 retry_count 加 1
retry_count += 1;
```
