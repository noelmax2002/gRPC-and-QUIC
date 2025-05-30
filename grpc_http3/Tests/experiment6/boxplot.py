import os
import re
import matplotlib.pyplot as plt
import numpy as np

def extract_data(file_name):
    y_vals = []
    with open(file_name, 'r') as file:
        for line in file:
            match = re.search(r"Total time elapsed:\s*(\d+\.\d+)", line)
            if match:
                value = float(match.group(1))
                y_vals.append(value)  
    return y_vals

def collect_folder_data(folder_path):
    all_y = []
    for file_name in os.listdir(folder_path):
        full_path = os.path.join(folder_path, file_name)
        if os.path.isfile(full_path):
            y_vals = extract_data(full_path)
            all_y.extend(y_vals)
    return all_y


#folder1 = "quic_latency"
#folder2 = "quic_latency_lrtt"
#folder3 = "quic_latency_normal"

folder1 = "Results/latency/last"
folder2 = "Results/latency/lrtt"
folder3 = "Results/latency/normal"

folder4 = "LossResults2/quic_latency_loss/last"
folder5 = "LossResults2/quic_latency_loss/lrtt"
folder6 = "LossResults2/quic_latency_loss/normal"

folder1 = "Results/cut/last"
folder2 = "Results/cut/lrtt"
folder3 = "Results/cut/normal"

folder4 = "LossResults2/quic_cut_loss/last"
folder5 = "LossResults2/quic_cut_loss/lrtt"
folder6 = "LossResults/quic_cut_loss/normal"


data1 = collect_folder_data(folder1)  
data2 = collect_folder_data(folder2)  
data3 = collect_folder_data(folder3)  
data4 = collect_folder_data(folder4)  
data5 = collect_folder_data(folder5)  
data6 = collect_folder_data(folder6) 

plt.figure(figsize=(8, 6))

box_data = [data1, data2, data3, data4, data5, data6]
positions = [1, 2, 3, 4, 5, 6]
colors = ['blue', 'green', 'purple', 'blue', 'green', 'purple'] 

boxplots = plt.boxplot(box_data, positions=positions, patch_artist=True, showfliers=False)

for patch, color in zip(boxplots['boxes'], colors):
    patch.set(facecolor='white', edgecolor=color, linewidth=2)

for i, (whisker, cap, median) in enumerate(zip(
        zip(boxplots['whiskers'][::2], boxplots['whiskers'][1::2]),
        zip(boxplots['caps'][::2], boxplots['caps'][1::2]),
        boxplots['medians'])):
    color = colors[i]
    for w in whisker:
        w.set(color=color, linewidth=1.5)
    for c in cap:
        c.set(color=color, linewidth=1.5)
    median.set(color=color, linewidth=2)


plt.xticks([1,2,3,4,5,6], ["|------", "------- Without loss -------", "------|", "|------", "------- With loss -------", "------|"])
plt.yticks(np.arange(19.5,25.5,0.5))
plt.ylabel('Total time of execution [s]')
plt.grid(True, axis='y')
plt.tight_layout()

custom_legend = [
    plt.Line2D([0], [0], color='blue', lw=2, label='Last scheduler'),
    plt.Line2D([0], [0], color='green', lw=2, label='Lrtt scheduler'),
    plt.Line2D([0], [0], color='purple', lw=2, label='Simple scheduler'),
]
plt.legend(handles=custom_legend, loc='upper left')

plt.savefig('Boxplot_cut.pdf')
plt.show()
