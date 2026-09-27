// 双形态：命令行(3.7 工具链×hvigor4，named appTasks) 与
// DevEco GUI(hvigor6，default.system) 各取所需。
import { appTasks } from '@ohos/hvigor-ohos-plugin';

export { appTasks };

export default {
    system: appTasks,
    plugins: []
}
