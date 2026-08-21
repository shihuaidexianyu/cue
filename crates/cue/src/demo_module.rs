//! Phase 1/2 的演示模块(§88):验证 Core 不含任何业务耦合。
//! 返回静态条目,支持子串过滤,activate 直接成功并请求记录 usage。

use cue_protocol::*;
use std::sync::Arc;

#[derive(Clone)]
struct DemoItem {
    name: String,
    subtitle: String,
}

pub struct DemoModule {
    descriptor: ModuleDescriptor,
    entries: Arc<Vec<DemoItem>>,
}

impl DemoModule {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                id: ModuleId::from_static("demo"),
                name: "Demo",
                version: "0.1.0",
            },
            entries: Arc::new(vec![
                DemoItem {
                    name: "Demo Notepad".into(),
                    subtitle: "Phase 1 fake entry".into(),
                },
                DemoItem {
                    name: "Demo Calculator".into(),
                    subtitle: "Phase 1 fake entry".into(),
                },
                DemoItem {
                    name: "Demo Zed".into(),
                    subtitle: "Phase 1 fake entry".into(),
                },
                DemoItem {
                    name: "永劫无间 (Demo)".into(),
                    subtitle: "验证 Unicode 渲染".into(),
                },
            ]),
        }
    }
}

impl Module for DemoModule {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn load(&mut self, ctx: ModuleContext) -> Result<(), ModuleError> {
        ctx.logger.log(LogLevel::Info, "DemoModule loaded");
        Ok(())
    }

    fn unload(&mut self) {}

    fn settings_schema(&self) -> SettingsSchema {
        Vec::new()
    }

    fn try_apply_settings(&mut self, _changes: SettingsChangeSet) -> Result<(), ModuleError> {
        Ok(())
    }
}

impl LauncherModule for DemoModule {
    fn launcher_descriptor(&self) -> LauncherDescriptor {
        LauncherDescriptor {
            trigger: None,
            is_default: true,
        }
    }

    fn query(&mut self, ctx: QueryContext) -> QueryFuture {
        let query = ctx.query.to_lowercase();
        let entries = Arc::clone(&self.entries);
        Box::pin(async move {
            let items = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| query.is_empty() || e.name.to_lowercase().contains(&query))
                .take(ctx.result_limit)
                .map(|(i, e)| ModuleItem::new(ItemId(i as u64), e.clone()))
                .collect();
            Ok(QueryResponse { items })
        })
    }

    fn present(&self, item: &ModuleItem) -> ResultPresentation {
        let Some(demo) = item.downcast_ref::<DemoItem>() else {
            return ResultPresentation::new("<unknown item>");
        };
        let mut p = ResultPresentation::new(demo.name.clone());
        p.subtitle = Some(demo.subtitle.clone().into());
        p.icon = Some(ResultIcon::SystemIcon(SystemIconId::App));
        p.badges.push(ResultBadge {
            text: "DEMO".into(),
        });
        p
    }

    fn actions(&self, _item: &ModuleItem) -> Vec<ActionDescriptor> {
        vec![ActionDescriptor {
            id: ActionId::PRIMARY,
            label: "Open (demo)".into(),
            shortcut: None,
        }]
    }

    fn activate(&mut self, item: &ModuleItem, action: ActionId) -> ActivationFuture {
        let name = item
            .downcast_ref::<DemoItem>()
            .map(|d| d.name.clone())
            .unwrap_or_default();
        Box::pin(async move {
            ModuleOutcome::success(
                SessionDisposition::Close,
                Some(UsageRecordRequest {
                    item_key: name,
                    action_id: action,
                }),
            )
        })
    }
}
